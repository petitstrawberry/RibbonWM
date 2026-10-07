#import <Cocoa/Cocoa.h>
#import <ApplicationServices/ApplicationServices.h>
#include <dlfcn.h>
#include "bridge.h"
#include "skylight.h"

static int (*connection)(void);
static uint64_t (*currentSpace)(int,CFStringRef);
static int (*spaceType)(int,uint64_t);
static CFArrayRef (*windowSpaces)(int,int,CFArrayRef);
static CGError (*axWindowId)(AXUIElementRef,uint32_t *);
static NSMutableDictionary<NSNumber *,NSDictionary *> *windowCache;
static CGError (*windowOwner)(int,uint32_t,int *);
static CGError (*connectionPID)(int,pid_t *);
static CGError (*surfaceBounds)(int,uint32_t,CGRect *);
static RibbonStickyAPI stickyAPI;
static bool hasStickyAPI;
// Behavior reference: yabai's AX_ENHANCED_UI_WORKAROUND. Preserve the owner's
// flag on every return path; Electron can animate otherwise synchronous AX writes.
@interface RibbonAXFrameGuard : NSObject {
    AXUIElementRef _application;
    BOOL _restore;
}
- (instancetype)initWithPID:(pid_t)pid;
@end
@implementation RibbonAXFrameGuard
- (instancetype)initWithPID:(pid_t)pid {
    self=[super init];if(!self)return nil;
    _application=AXUIElementCreateApplication(pid);AXUIElementSetMessagingTimeout(_application,0.05);
    CFTypeRef value=NULL;
    if(!AXUIElementCopyAttributeValue(_application,CFSTR("AXEnhancedUserInterface"),&value)&&value==kCFBooleanTrue)
        _restore=!AXUIElementSetAttributeValue(_application,CFSTR("AXEnhancedUserInterface"),kCFBooleanFalse);
    if(value)CFRelease(value);return self;
}
- (void)dealloc {
    if(_restore)AXUIElementSetAttributeValue(_application,CFSTR("AXEnhancedUserInterface"),kCFBooleanTrue);
    if(_application)CFRelease(_application);
}
@end
static void resolve(void) {
    static dispatch_once_t once;
    dispatch_once(&once,^{
        windowCache=[NSMutableDictionary dictionary];
        void *h=dlopen("/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight",RTLD_NOW);
        connection=dlsym(h,"SLSMainConnectionID");currentSpace=dlsym(h,"SLSManagedDisplayGetCurrentSpace");
        spaceType=dlsym(h,"SLSSpaceGetType");windowSpaces=dlsym(h,"SLSCopySpacesForWindows");
        axWindowId=dlsym(RTLD_DEFAULT,"_AXUIElementGetWindow");
        windowOwner=dlsym(h,"SLSGetWindowOwner");connectionPID=dlsym(h,"SLSConnectionGetPID");
        surfaceBounds=dlsym(h,"SLSGetWindowBounds");
        hasStickyAPI=loadStickyAPI(&stickyAPI);
    });
}
static NSDictionary *rectJSON(CGRect r) {
    // An empty WindowServer clip can report CGRectNull (infinite origin), even
    // when the query succeeds. Never turn that sentinel into JSON numbers.
    if(CGRectIsNull(r)||CGRectIsInfinite(r)||!isfinite(r.origin.x)||!isfinite(r.origin.y)||!isfinite(r.size.width)||!isfinite(r.size.height)||
        r.size.width<0||r.size.height<0)return nil;
    return @{@"x":@(r.origin.x),@"y":@(r.origin.y),@"width":@(r.size.width),@"height":@(r.size.height)};
}
static BOOL usableBounds(CGRect r) {
    return rectJSON(r)!=nil&&r.size.width>0&&r.size.height>0;
}
static char *queryJSON(int kind,CFArrayRef selected) { @autoreleasepool {
    resolve(); NSMutableArray *rows=[NSMutableArray array];
    if (kind==0) {
        NSArray<NSScreen *> *screens=NSScreen.screens;
        // Secondary visibleFrame can include the menu bar (observed on macOS 26).
        // Reserve the primary screen's measured bar height on every display;
        // NSStatusBar.thickness alone reports 22 even when the visible inset is 30.
        CGFloat menuHeight=NSStatusBar.systemStatusBar.thickness;
        if(screens.count)menuHeight=MAX(menuHeight,NSMaxY(screens[0].frame)-NSMaxY(screens[0].visibleFrame));
        for (NSScreen *screen in screens) {
            CGDirectDisplayID did=[screen.deviceDescription[@"NSScreenNumber"] unsignedIntValue];
            CFUUIDRef uuid=CGDisplayCreateUUIDFromDisplayID(did);
            CFStringRef uuidString=uuid?CFUUIDCreateString(NULL,uuid):NULL;
            if (!uuidString) {if(uuid)CFRelease(uuid);continue;}
            uint64_t sid=connection&&currentSpace?currentSpace(connection(),uuidString):0;
            NSRect v=screen.visibleFrame;
            CGRect frame=CGDisplayBounds(did);
            // Convert screen-local insets: AppKit is bottom-up, CG/AX top-down.
            // Displays above primary can have a negative global CG origin.
            NSRect a=screen.frame;
            RibbonRect usable=ribbon_display_viewport((RibbonRect){a.origin.x,a.origin.y,a.size.width,a.size.height},
                (RibbonRect){v.origin.x,v.origin.y,v.size.width,v.size.height},
                (RibbonRect){frame.origin.x,frame.origin.y,frame.size.width,frame.size.height},MAX(menuHeight,screen.safeAreaInsets.top));
            CGRect viewport=CGRectMake(usable.x,usable.y,usable.width,usable.height);
            if(!rectJSON(frame)||!rectJSON(viewport)) {CFRelease(uuidString);CFRelease(uuid);continue;}
            [rows addObject:@{@"id":(__bridge NSString *)uuidString,@"display_id":@(did),@"name":screen.localizedName,
                @"frame":rectJSON(frame),@"viewport":rectJSON(viewport),
                @"scale":@(screen.backingScaleFactor),@"native_space":@(sid),@"native_fullscreen":(spaceType&&sid&&spaceType(connection(),sid)!=0)?@YES:@NO,@"primary":CGDisplayIsMain(did)?@YES:@NO}];
            CFRelease(uuidString);CFRelease(uuid);
        }
    } else if(kind==2) {
        for(NSRunningApplication *app in NSWorkspace.sharedWorkspace.runningApplications)
            if(app.activationPolicy==NSApplicationActivationPolicyRegular&&!app.terminated)
                [rows addObject:@{@"pid":@(app.processIdentifier),@"app":app.localizedName?:@"",@"bundle_id":app.bundleIdentifier?:@""}];
    } else {
        NSMutableDictionary<NSNumber *,NSString *> *bundles=[NSMutableDictionary dictionary];
        CFArrayRef windows=selected?CFRetain(selected):CGWindowListCopyWindowInfo(kCGWindowListOptionAll|kCGWindowListExcludeDesktopElements,kCGNullWindowID);
        if(!windows)return NULL;
        for (NSDictionary *w in (__bridge NSArray *)windows) {
            CGRect b; if (!CGRectMakeWithDictionaryRepresentation((__bridge CFDictionaryRef)w[(id)kCGWindowBounds],&b)) continue;
            NSDictionary *presented=rectJSON(b);
            if(!presented)continue;
            uint32_t wid=[w[(id)kCGWindowNumber] unsignedIntValue];
            NSNumber *owner=w[(id)kCGWindowOwnerPID]?:@0;
            NSString *bundle=bundles[owner];
            if(!bundle) {
                bundle=[NSRunningApplication runningApplicationWithProcessIdentifier:owner.intValue].bundleIdentifier?:@"";
                bundles[owner]=bundle;
            }
            CFArrayRef spaces=kind!=3&&connection&&windowSpaces?windowSpaces(connection(),7,(__bridge CFArrayRef)@[@(wid)]):NULL;
            bool sticky=false;
            bool stickyKnown=kind!=3&&hasStickyAPI&&connection&&readSticky(&stickyAPI,connection(),wid,&sticky);
            CGRect surface=CGRectNull;
            id physical=kind!=3&&surfaceBounds&&connection&&!surfaceBounds(connection(),wid,&surface)?rectJSON(surface):nil;
            [rows addObject:@{@"id":@(wid),@"pid":w[(id)kCGWindowOwnerPID]?:@0,@"app":w[(id)kCGWindowOwnerName]?:@"",
                @"title":w[(id)kCGWindowName]?:@"",@"layer":w[(id)kCGWindowLayer]?:@0,@"onscreen":[w[(id)kCGWindowIsOnscreen] boolValue]?@YES:@NO,
                @"bundle_id":bundle,
                @"bounds":presented,@"surface_bounds":physical?:NSNull.null,@"native_spaces":spaces?(__bridge NSArray *)spaces:@[],
                @"sticky":@(sticky),@"sticky_known":@(stickyKnown)}];
            if(spaces)CFRelease(spaces);
        }
        if(windows)CFRelease(windows);
    }
    // Do not let an Objective-C serialization exception unwind through Rust.
    NSData *json=nil;
    @try {json=[NSJSONSerialization dataWithJSONObject:rows options:0 error:nil];}
    @catch(NSException *exception) {fprintf(stderr,"Window inventory serialization: %s\n",exception.reason.UTF8String);return NULL;}
    if(!json)return NULL;
    char *result=malloc(json.length+1);if(!result)return NULL;memcpy(result,json.bytes,json.length);result[json.length]=0;return result;
} }
char *ribbon_query_json(int kind) {return queryJSON(kind,NULL);}
char *ribbon_query_windows_json(const uint32_t *ids,size_t count) { @autoreleasepool {
    // CGWindowList arrays contain raw CGWindowID values, not CFNumbers.
    CFMutableArrayRef numbers=CFArrayCreateMutable(NULL,(CFIndex)count,NULL);
    for(size_t i=0;i<count;i++)CFArrayAppendValue(numbers,(const void *)(uintptr_t)ids[i]);
    CFArrayRef descriptions=CGWindowListCreateDescriptionFromArray(numbers);CFRelease(numbers);
    if(!descriptions)return NULL;
    char *result=queryJSON(1,descriptions);CFRelease(descriptions);return result;
} }
void ribbon_free(void *pointer) {free(pointer);}
static BOOL activeSession(NSDictionary *session) {
    return session && [session[(__bridge NSString *)kCGSessionOnConsoleKey] boolValue] &&
        ![session[@"CGSSessionScreenIsLocked"] boolValue];
}
int ribbon_session_active(void) { @autoreleasepool {
    CFDictionaryRef session=CGSessionCopyCurrentDictionary();
    BOOL active=activeSession((__bridge NSDictionary *)session);
    if(session)CFRelease(session);return active;
} }
int ribbon_ax_trusted(void) { @autoreleasepool {
    // Refresh without reopening the asynchronous permission prompt.
    NSDictionary *options=@{(__bridge NSString *)kAXTrustedCheckOptionPrompt:@NO};
    return AXIsProcessTrustedWithOptions((__bridge CFDictionaryRef)options);
} }
void ribbon_permission_host_initialize(void) {
    [NSApplication sharedApplication];[NSApp setActivationPolicy:NSApplicationActivationPolicyAccessory];
    [NSApp finishLaunching];
}
int ribbon_owned_probe_accessible(int pid) { @autoreleasepool {
    if(pid<=0)return 0;
    AXUIElementRef app=AXUIElementCreateApplication(pid);CFTypeRef windows=NULL;
    AXUIElementSetMessagingTimeout(app,0.05);
    AXError error=AXUIElementCopyAttributeValue(app,kAXWindowsAttribute,&windows);
    BOOL accessible=!error&&windows&&CFGetTypeID(windows)==CFArrayGetTypeID();
    if(windows)CFRelease(windows);CFRelease(app);return accessible;
} }
static void permission_wait_timer(CFRunLoopTimerRef timer,void *context) {(void)timer;(void)context;}
void ribbon_wait_for_events(double seconds) {
    double duration=fmin(1,fmax(0,seconds));
    CFRunLoopRef loop=CFRunLoopGetCurrent();
    // A service waiting before AppKit/AX observers exist can have no sources.
    // Keep the run loop alive until the deadline instead of spinning on Finished.
    CFRunLoopTimerRef timer=CFRunLoopTimerCreate(NULL,CFAbsoluteTimeGetCurrent()+duration,0,0,0,permission_wait_timer,NULL);
    CFRunLoopAddTimer(loop,timer,kCFRunLoopDefaultMode);
    CFRunLoopRunInMode(kCFRunLoopDefaultMode,duration,true);
    CFRunLoopRemoveTimer(loop,timer,kCFRunLoopDefaultMode);CFRelease(timer);
}
int ribbon_ax_request_permission(void) { @autoreleasepool {
    NSDictionary *options=@{(__bridge NSString *)kAXTrustedCheckOptionPrompt:@YES};
    return AXIsProcessTrustedWithOptions((__bridge CFDictionaryRef)options);
} }
uint32_t ribbon_ax_window_id(const void *element) {
    resolve();uint32_t wid=0;
    if(axWindowId&&element&&!axWindowId((AXUIElementRef)element,&wid))return wid;
    return 0;
}
static AXUIElementRef findAXWindow(uint32_t wid,int expected_pid,pid_t *pid) {
    resolve();if(!axWindowId)return NULL;
    // Exact WindowServer ownership works across Spaces without enumerating
    // every desktop window for every AX operation.
    *pid=ribbon_window_owner(wid);
    if(*pid!=expected_pid)return NULL;
    // AXWindowID lookup can stop resolving while its native Space is inactive.
    // Retain the already resolved AX element; guard it against current WS ownership.
    NSDictionary *cached=windowCache[@(wid)];
    if([cached[@"pid"] intValue]==expected_pid&&cached[@"element"])
        return (AXUIElementRef)CFRetain((__bridge CFTypeRef)cached[@"element"]);
    [windowCache removeObjectForKey:@(wid)];
    AXUIElementRef app=AXUIElementCreateApplication(*pid);CFTypeRef windows=NULL;
    AXUIElementSetMessagingTimeout(app,0.25);
    AXError error=AXUIElementCopyAttributeValue(app,kAXWindowsAttribute,&windows);CFRelease(app);
    if(error||!windows)return NULL;
    AXUIElementRef found=NULL;
    for(CFIndex i=0;i<CFArrayGetCount(windows);i++) {
        AXUIElementRef w=(AXUIElementRef)CFArrayGetValueAtIndex(windows,i);uint32_t id=0;
        if(!axWindowId(w,&id)&&id==wid){found=(AXUIElementRef)CFRetain(w);AXUIElementSetMessagingTimeout(found,0.25);break;}
    }
    CFRelease(windows);
    if(found)windowCache[@(wid)]=@{@"pid":@(expected_pid),@"element":(__bridge id)found};
    return found;
}
static AXError axGeometry(AXUIElementRef window,RibbonRect *rect) {
    CFTypeRef p=NULL,s=NULL;CGPoint point=CGPointZero;CGSize size=CGSizeZero;
    AXError error=AXUIElementCopyAttributeValue(window,kAXPositionAttribute,&p);
    if(!error)error=AXUIElementCopyAttributeValue(window,kAXSizeAttribute,&s);
    if(!error&&(!p||!s||!AXValueGetValue(p,kAXValueCGPointType,&point)||!AXValueGetValue(s,kAXValueCGSizeType,&size)))error=kAXErrorFailure;
    if(p)CFRelease(p);if(s)CFRelease(s);
    if(!error)*rect=(RibbonRect){point.x,point.y,size.width,size.height};
    return error;
}
// Read-only discovery/membership probe. Runs on the inventory worker and
// deliberately does not touch the main-thread AX cache or observer tables.
char *ribbon_probe_application(int pid,const uint32_t *candidates,size_t count) { @autoreleasepool {
    if(!ribbon_session_active())return NULL;
    resolve();if(pid<=0||!axWindowId||count>512)return NULL;
    AXUIElementRef app=AXUIElementCreateApplication(pid);CFTypeRef windows=NULL;
    AXUIElementSetMessagingTimeout(app,0.05);
    AXError error=AXUIElementCopyAttributeValue(app,kAXWindowsAttribute,&windows);CFRelease(app);
    if(error||!windows||CFGetTypeID(windows)!=CFArrayGetTypeID()){if(windows)CFRelease(windows);return NULL;}
    NSMutableArray *members=[NSMutableArray array],*ready=[NSMutableArray array];
    BOOL complete=YES;
    SkyLight sky;BOOL hasSky=loadSkyLight(&sky);
    for(id object in (__bridge NSArray *)windows) {
        AXUIElementRef w=(__bridge AXUIElementRef)object;uint32_t wid=0;
        AXUIElementSetMessagingTimeout(w,0.05);
        // An incomplete enumeration cannot prove that an old window closed.
        if(axWindowId(w,&wid)||!wid){complete=NO;continue;}
        [members addObject:@(wid)];BOOL candidate=NO;
        for(size_t i=0;i<count;i++)if(candidates[i]==wid){candidate=YES;break;}
        if(!candidate||ribbon_window_owner(wid)!=pid)continue;
        CFTypeRef subrole=NULL;Boolean size=0,position=0;
        AXError sub=AXUIElementCopyAttributeValue(w,kAXSubroleAttribute,&subrole);
        BOOL standard=!sub&&subrole&&CFEqual(subrole,kAXStandardWindowSubrole);
        if(subrole)CFRelease(subrole);
        if(!standard||AXUIElementIsAttributeSettable(w,kAXSizeAttribute,&size)||!size||
            AXUIElementIsAttributeSettable(w,kAXPositionAttribute,&position)||!position)continue;
        RibbonRect logical;CGAffineTransform transform;
        if(axGeometry(w,&logical)||!hasSky||sky.getTransform(sky.connection(),wid,&transform)||
            !isfinite(transform.a)||!isfinite(transform.b)||!isfinite(transform.c)||!isfinite(transform.d)||
            fabs(transform.a-1)>1e-6||fabs(transform.b)>1e-6||fabs(transform.c)>1e-6||fabs(transform.d-1)>1e-6||
            !isfinite(transform.tx)||!isfinite(transform.ty))continue;
        CGRect r=CGRectMake(logical.x,logical.y,logical.width,logical.height);
        if(!usableBounds(r))continue;
        [ready addObject:@{@"id":@(wid),@"geometry":rectJSON(r)}];
    }
    CFRelease(windows);
    NSData *data=[NSJSONSerialization dataWithJSONObject:@{@"members":complete?members:NSNull.null,@"ready":ready} options:0 error:nil];
    if(!data)return NULL;char *result=malloc(data.length+1);if(!result)return NULL;
    memcpy(result,data.bytes,data.length);result[data.length]=0;return result;
} }
int ribbon_window_geometry(uint32_t wid,int expected_pid,RibbonRect *rect) { @autoreleasepool {
    pid_t pid=0;AXUIElementRef window=findAXWindow(wid,expected_pid,&pid);
    if(!window)return kAXErrorInvalidUIElement;
    AXError error=axGeometry(window,rect);CFRelease(window);return error;
} }
int ribbon_window_presentation(uint32_t wid,int expected_pid,RibbonRect *rect,RibbonRect *surface) {
    if(ribbon_window_owner(wid)!=expected_pid)return kAXErrorInvalidUIElement;
    SkyLight sky;CGRect bounds;CGAffineTransform transform;
    if(!loadSkyLight(&sky)||sky.getBounds(sky.connection(),wid,&bounds)||sky.getTransform(sky.connection(),wid,&transform)||
        !usableBounds(bounds)||!isfinite(transform.a)||!isfinite(transform.b)||!isfinite(transform.c)||!isfinite(transform.d)||
        !isfinite(transform.tx)||!isfinite(transform.ty)||fabs(transform.a-1)>1e-6||
        fabs(transform.b)>1e-6||fabs(transform.c)>1e-6||fabs(transform.d-1)>1e-6)return kAXErrorFailure;
    *rect=(RibbonRect){-transform.tx,-transform.ty,bounds.size.width,bounds.size.height};
    *surface=(RibbonRect){bounds.origin.x,bounds.origin.y,bounds.size.width,bounds.size.height};return 0;
}
typedef struct {
    AXUIElementRef window;
    CFTypeRef guard;
    uint32_t wid;
    pid_t pid;
    RibbonRect rect,outer;
    BOOL exactPosition,havePrevious;
    CGAffineTransform previous;
    double deadline,stable;
} RibbonSettlement;
static RibbonSettlement *newSettlement(AXUIElementRef window,uint32_t wid,RibbonRect rect,RibbonRect outer,BOOL exactPosition) {
    RibbonSettlement *state=calloc(1,sizeof(*state));if(!state)return NULL;
    state->window=(AXUIElementRef)CFRetain(window);AXUIElementGetPid(window,&state->pid);
    AXUIElementSetMessagingTimeout(window,0.01);
    state->wid=wid;state->rect=rect;state->outer=outer;
    state->exactPosition=exactPosition;state->deadline=NSProcessInfo.processInfo.systemUptime+0.5;return state;
}
void ribbon_settlement_release(void *context) {
    RibbonSettlement *state=context;if(!state)return;
    AXUIElementSetMessagingTimeout(state->window,0.25);
    CFRelease(state->window);if(state->guard)CFRelease(state->guard);free(state);
}
// Pure temporal check: every invalid sample resets the stable interval.
static int sampleSettlement(RibbonSettlement *state,BOOL matches,CGAffineTransform transform,double now) {
    if(now>=state->deadline)return kAXErrorCannotComplete;
    if(matches&&state->havePrevious&&CGAffineTransformEqualToTransform(transform,state->previous)) {
        if(!state->stable)state->stable=now;
        if(now-state->stable>=0.05)return 1;
    } else state->stable=0;
    state->previous=transform;state->havePrevious=matches;
    return 0;
}
int ribbon_settlement_poll(void *context) { @autoreleasepool {
    RibbonSettlement *state=context;if(!state)return kAXErrorIllegalArgument;
    if(!ribbon_session_active()||ribbon_left_mouse_down())return kAXErrorCannotComplete;
    if(ribbon_window_owner(state->wid)!=state->pid)return kAXErrorInvalidUIElement;
    SkyLight sky;if(!loadSkyLight(&sky))return kAXErrorFailure;
    CGRect bounds;CGAffineTransform transform;
    CGError eb=sky.getBounds(sky.connection(),state->wid,&bounds),et=sky.getTransform(sky.connection(),state->wid,&transform);
    if(eb||et||!usableBounds(bounds))return kAXErrorCannotComplete;
    RibbonRect logical={0};AXError ax=axGeometry(state->window,&logical);
    if(ax) {
        state->stable=0;state->havePrevious=NO;
        return ax==kAXErrorCannotComplete&&NSProcessInfo.processInfo.systemUptime<state->deadline?0:ax;
    }
    // AX positions need not equal SLS bounds (Chrome titlebar offsets).
    // Verify the owner's position in the coordinate system we wrote.
    BOOL position=fabs(logical.x-state->rect.x)<=2&&fabs(logical.y-state->rect.y)<=2;
    if(!state->exactPosition) {
        // AppKit may constrain the logical anchor. Visual placement belongs
        // to Dock, so accept a stable anchor inside the intended display.
        for(NSScreen *screen in NSScreen.screens) {
            CGRect display=CGDisplayBounds([screen.deviceDescription[@"NSScreenNumber"] unsignedIntValue]);
            if(CGRectContainsPoint(display,CGPointMake(state->rect.x,state->rect.y))) {
                position=CGRectContainsPoint(display,CGPointMake(logical.x,logical.y));break;
            }
        }
    }
    BOOL matches=position&&fabs(logical.width-state->rect.width)<=2&&fabs(logical.height-state->rect.height)<=2&&
        fabs(bounds.size.width-state->outer.width)<=2&&fabs(bounds.size.height-state->outer.height)<=2;
    return sampleSettlement(state,matches,transform,NSProcessInfo.processInfo.systemUptime);
} }
static int settleWindow(AXUIElementRef window,uint32_t wid,RibbonRect rect,RibbonRect outer,BOOL exactPosition,RibbonGeometryProgress progress,void *context) {
    RibbonSettlement *state=newSettlement(window,wid,rect,outer,exactPosition);
    if(!state)return kAXErrorFailure;
    int result;
    do {
        if(progress&&progress(context)){result=kAXErrorFailure;break;}
        result=ribbon_settlement_poll(state);
        if(!result)usleep(5000);
    } while(!result);
    ribbon_settlement_release(state);return result==1?0:result;
}
static AXError acceptedSize(AXUIElementRef window,CGSize expected,CGSize *accepted,RibbonGeometryProgress progress,void *context) {
    double deadline=NSProcessInfo.processInfo.systemUptime+0.25;
    for(;;) {
        if(progress&&progress(context))return kAXErrorFailure;
        CFTypeRef actual=NULL;AXError error=AXUIElementCopyAttributeValue(window,kAXSizeAttribute,&actual);
        if(!error&&(!actual||!AXValueGetValue(actual,kAXValueCGSizeType,accepted)))error=kAXErrorFailure;
        if(actual)CFRelease(actual);
        if(error)return error;
        if(fabs(accepted->width-expected.width)<=2&&fabs(accepted->height-expected.height)<=2)return 0;
        if(NSProcessInfo.processInfo.systemUptime>=deadline)return kAXErrorCannotComplete;
        usleep(5000);
    }
}
static int resizeWindow(uint32_t wid,int expected_pid,RibbonRect target,BOOL logicalTarget,RibbonGeometryProgress progress,void *context,void **pending) { @autoreleasepool {
    if(!ribbon_session_active())return kAXErrorCannotComplete;
    pid_t pid=0;AXUIElementRef w=findAXWindow(wid,expected_pid,&pid);if(!w)return kAXErrorInvalidUIElement;
    __attribute__((objc_precise_lifetime)) RibbonAXFrameGuard *guard=[[RibbonAXFrameGuard alloc] initWithPID:pid];
    (void)guard;
    SkyLight sky;CGRect bounds;RibbonRect ax={0};
    AXError snapshot=axGeometry(w,&ax);
    if(snapshot||!loadSkyLight(&sky)||sky.getBounds(sky.connection(),wid,&bounds)||!usableBounds(bounds)){CFRelease(w);return snapshot?snapshot:kAXErrorFailure;}
    RibbonRect native={bounds.origin.x,bounds.origin.y,bounds.size.width,bounds.size.height};
    // AX can describe an inset client frame. Layout/clipping describes the
    // outer WindowServer surface. Convert both origin and size through the
    // measured per-window difference rather than assuming identical frames.
    RibbonRect rect=logicalTarget?target:ribbon_outer_to_ax(target,ax,native);
    RibbonRect outer=logicalTarget?(RibbonRect){target.x+native.x-ax.x,target.y+native.y-ax.y,
        target.width+native.width-ax.width,target.height+native.height-ax.height}:target;
    if(!isfinite(rect.x)||!isfinite(rect.y)||!isfinite(rect.width)||!isfinite(rect.height)||rect.width<=0||rect.height<=0){CFRelease(w);return kAXErrorIllegalArgument;}
    // A press can arrive while an earlier AX request is settling. Never issue
    // a new position/size write into the owner's native mouse interaction.
    if(ribbon_left_mouse_down()){CFRelease(w);return kAXErrorCannotComplete;}
    // AppKit constrains a resize against the logical frame's current position.
    // Anchor it inside its monitor before sizing; the compositor owns the visual position.
    CGPoint position=CGPointMake(rect.x,rect.y);AXValueRef p=AXValueCreate(kAXValueCGPointType,&position);
    AXError error=0;
    if(fabs(ax.x-position.x)>2||fabs(ax.y-position.y)>2)error=AXUIElementSetAttributeValue(w,kAXPositionAttribute,p);
    CFRelease(p);
    if(!error&&progress&&progress(context))error=kAXErrorFailure;
    CGSize size=CGSizeMake(rect.width,rect.height);AXValueRef value=AXValueCreate(kAXValueCGSizeType,&size);
    if(!error&&ribbon_left_mouse_down())error=kAXErrorCannotComplete;
    if(!error&&(fabs(ax.width-size.width)>2||fabs(ax.height-size.height)>2))error=AXUIElementSetAttributeValue(w,kAXSizeAttribute,value);
    CFRelease(value);
    CGSize accepted=CGSizeZero;
    if(!error)error=acceptedSize(w,size,&accepted,progress,context);
    // Do not manufacture a smaller intermediate size to force acceptance.
    // A refusal is reported as-is; normal focus/scroll never enters this API.
    // Position writes can overwrite an owner-side pending resize with its old
    // frame. Re-anchor only after the new size has actually been accepted.
    RibbonRect after={0};
    if(!error)error=axGeometry(w,&after);
    p=AXValueCreate(kAXValueCGPointType,&position);
    if(!error&&(fabs(after.x-position.x)>2||fabs(after.y-position.y)>2))error=AXUIElementSetAttributeValue(w,kAXPositionAttribute,p);
    CFRelease(p);
    if(error)fprintf(stderr,"Window %u AX resize failed (%d): requested=(%.1f,%.1f), accepted=(%.1f,%.1f)\n",wid,error,size.width,size.height,accepted.width,accepted.height);
    if(!error&&pending) {
        RibbonSettlement *state=newSettlement(w,wid,rect,outer,NO);
        if(state){state->guard=CFBridgingRetain(guard);*pending=state;}
        else error=kAXErrorFailure;
    } else if(!error)error=settleWindow(w,wid,rect,outer,NO,progress,context);
    CFRelease(w);return error;
} }
int ribbon_resize_begin(uint32_t wid,int expected_pid,RibbonRect rect,RibbonGeometryProgress progress,void *context,void **pending) {
    *pending=NULL;return resizeWindow(wid,expected_pid,rect,NO,progress,context,pending);
}
int ribbon_resize_window(uint32_t wid,int expected_pid,RibbonRect rect) {
    return ribbon_resize_window_observed(wid,expected_pid,rect,NULL,NULL);
}
int ribbon_resize_window_observed(uint32_t wid,int expected_pid,RibbonRect rect,RibbonGeometryProgress progress,void *context) {
    int error=resizeWindow(wid,expected_pid,rect,NO,progress,context,NULL);
    // Owner-side chrome can change its inset during the first resize. Measure
    // it again once; a genuine minimum-size refusal still remains an error.
    if(error==kAXErrorCannotComplete)error=resizeWindow(wid,expected_pid,rect,NO,progress,context,NULL);
    return error;
}
int ribbon_restore_window(uint32_t wid,int expected_pid,RibbonRect rect) { @autoreleasepool {
    int resized=resizeWindow(wid,expected_pid,rect,YES,NULL,NULL,NULL);if(resized)return resized;
    pid_t pid=0;AXUIElementRef w=findAXWindow(wid,expected_pid,&pid);if(!w)return kAXErrorInvalidUIElement;
    // Reapply the exact original position only after the size has settled, so
    // AppKit doesn't constrain it against the larger managed size.
    CGPoint point=CGPointMake(rect.x,rect.y);AXValueRef p=AXValueCreate(kAXValueCGPointType,&point);
    AXError ep=AXUIElementSetAttributeValue(w,kAXPositionAttribute,p);
    // Restoration targets the saved AX frame; the settled surface may include
    // owner chrome outside it. Preserve that accepted outer size here.
    SkyLight sky;CGRect b=CGRectZero;
    if(!ep&&(!loadSkyLight(&sky)||sky.getBounds(sky.connection(),wid,&b)))ep=kAXErrorFailure;
    RibbonRect outer={b.origin.x,b.origin.y,b.size.width,b.size.height};
    CFRelease(p);if(!ep)ep=settleWindow(w,wid,rect,outer,YES,NULL,NULL);
    CFRelease(w);return ep;
} }
int ribbon_focus_window(uint32_t wid,int expected_pid) { @autoreleasepool {
    pid_t pid=0;AXUIElementRef w=findAXWindow(wid,expected_pid,&pid);if(!w)return kAXErrorInvalidUIElement;
    NSRunningApplication *app=[NSRunningApplication runningApplicationWithProcessIdentifier:pid];
    [app activateWithOptions:0];
    // Background activation requests may be ignored by modern macOS. AX
    // explicitly fronts the already validated owner, then raises its window.
    AXUIElementRef application=AXUIElementCreateApplication(pid);
    AXUIElementSetMessagingTimeout(application,0.25);
    AXError error=AXUIElementSetAttributeValue(application,kAXFrontmostAttribute,kCFBooleanTrue);
    CFRelease(application);
    if(!error)error=AXUIElementPerformAction(w,kAXRaiseAction);CFRelease(w);return error;
} }
int ribbon_frontmost_pid(void) { @autoreleasepool {
    NSWorkspace *workspace=NSWorkspace.sharedWorkspace;
    // Rust owns the main loop rather than NSApplication.run. NSWorkspace's
    // cached frontmost application needs its pending notifications delivered;
    // otherwise it keeps reporting the app that was active at startup.
    CFRunLoopRunInMode(kCFRunLoopDefaultMode,0,true);
    return workspace.frontmostApplication.processIdentifier;
} }
uint32_t ribbon_focused_window(int expected_pid) { @autoreleasepool {
    resolve();
    if(!axWindowId||expected_pid<=0||ribbon_frontmost_pid()!=expected_pid)return 0;
    AXUIElementRef app=AXUIElementCreateApplication(expected_pid);CFTypeRef window=NULL;
    AXUIElementSetMessagingTimeout(app,0.05);
    AXError error=AXUIElementCopyAttributeValue(app,kAXFocusedWindowAttribute,&window);CFRelease(app);
    uint32_t wid=0;
    if(!error&&window) {
        AXUIElementSetMessagingTimeout((AXUIElementRef)window,0.05);
        if(axWindowId((AXUIElementRef)window,&wid))wid=0;
    }
    if(window)CFRelease(window);return wid;
} }
int ribbon_window_manageable(uint32_t wid,int expected_pid) { @autoreleasepool {
    pid_t pid=0;AXUIElementRef window=findAXWindow(wid,expected_pid,&pid);if(!window)return 0;
    CFTypeRef subrole=NULL;Boolean size=0,position=0;
    AXError e=AXUIElementCopyAttributeValue(window,kAXSubroleAttribute,&subrole);
    BOOL standard=!e&&subrole&&CFEqual(subrole,kAXStandardWindowSubrole);
    if(subrole)CFRelease(subrole);
    BOOL settable=standard&&!AXUIElementIsAttributeSettable(window,kAXSizeAttribute,&size)&&size&&
        !AXUIElementIsAttributeSettable(window,kAXPositionAttribute,&position)&&position;
    // Enroll only after the opening animation has settled, using the same
    // ordinary-transform contract as the Dock snapshot. Already managed
    // windows do not pass through this discovery-only check.
    if(settable) {
        SkyLight sky;CGRect bounds;CGAffineTransform transform;
        settable=loadSkyLight(&sky)&&!sky.getBounds(sky.connection(),wid,&bounds)&&
            !sky.getTransform(sky.connection(),wid,&transform)&&fabs(transform.a-1)<1e-6&&
            fabs(transform.b)<1e-6&&fabs(transform.c)<1e-6&&fabs(transform.d-1)<1e-6&&
            isfinite(transform.tx)&&isfinite(transform.ty);
        if(settable) {
            BOOL found=NO;
            CFArrayRef list=CGWindowListCopyWindowInfo(kCGWindowListOptionAll|kCGWindowListExcludeDesktopElements,kCGNullWindowID);
            for(NSDictionary *row in (__bridge NSArray *)list) {
                if([row[(id)kCGWindowNumber] unsignedIntValue]!=wid)continue;
                CGRect presented;
                found=CGRectMakeWithDictionaryRepresentation((__bridge CFDictionaryRef)row[(id)kCGWindowBounds],&presented)&&
                    fabs(presented.size.width-bounds.size.width)<=2&&fabs(presented.size.height-bounds.size.height)<=2;
                break;
            }
            if(list)CFRelease(list);settable=found;
        }
    }
    if(settable)ribbon_watch_ax_element(window,expected_pid);
    CFRelease(window);return settable;
} }
int ribbon_window_owner(uint32_t wid) { @autoreleasepool {
    resolve();int cid=0;pid_t pid=0;
    if(connection&&windowOwner&&connectionPID)
        return windowOwner(connection(),wid,&cid)||connectionPID(cid,&pid)?0:pid;
    return 0;
} }
void ribbon_forget_window(uint32_t wid) { @autoreleasepool {
    resolve();[windowCache removeObjectForKey:@(wid)];
    ribbon_forget_watched_window(wid);
} }
void ribbon_pointer(double *x,double *y) {
    CGEventRef e=CGEventCreate(NULL);CGPoint p=CGEventGetLocation(e);*x=p.x;*y=p.y;CFRelease(e);
}
int ribbon_left_mouse_down(void) {
    return CGEventSourceButtonState(kCGEventSourceStateCombinedSessionState,kCGMouseButtonLeft);
}
double ribbon_left_mouse_down_age(void) {
    return CGEventSourceSecondsSinceLastEventType(kCGEventSourceStateCombinedSessionState,kCGEventLeftMouseDown);
}
