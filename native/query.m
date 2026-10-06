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
static RibbonStickyAPI stickyAPI;
static bool hasStickyAPI;
static void resolve(void) {
    static dispatch_once_t once;
    dispatch_once(&once,^{
        windowCache=[NSMutableDictionary dictionary];
        void *h=dlopen("/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight",RTLD_NOW);
        connection=dlsym(h,"SLSMainConnectionID");currentSpace=dlsym(h,"SLSManagedDisplayGetCurrentSpace");
        spaceType=dlsym(h,"SLSSpaceGetType");windowSpaces=dlsym(h,"SLSCopySpacesForWindows");
        axWindowId=dlsym(RTLD_DEFAULT,"_AXUIElementGetWindow");
        windowOwner=dlsym(h,"SLSGetWindowOwner");connectionPID=dlsym(h,"SLSConnectionGetPID");
        hasStickyAPI=loadStickyAPI(&stickyAPI);
    });
}
static NSDictionary *rectJSON(CGRect r) {return @{@"x":@(r.origin.x),@"y":@(r.origin.y),@"width":@(r.size.width),@"height":@(r.size.height)};}
char *ribbon_query_json(int kind) { @autoreleasepool {
    resolve(); NSMutableArray *rows=[NSMutableArray array];
    if (kind==0) {
        NSArray<NSScreen *> *screens=NSScreen.screens;
        CGFloat primaryHeight=screens.count?screens[0].frame.size.height:0;
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
            CGFloat top=MAX(primaryHeight-NSMaxY(v),frame.origin.y+MAX(menuHeight,screen.safeAreaInsets.top));
            CGFloat bottom=primaryHeight-NSMinY(v);
            CGRect viewport=CGRectMake(v.origin.x,top,v.size.width,MAX(1,bottom-top));
            [rows addObject:@{@"id":(__bridge NSString *)uuidString,@"display_id":@(did),@"name":screen.localizedName,
                @"frame":rectJSON(frame),@"viewport":rectJSON(viewport),
                @"scale":@(screen.backingScaleFactor),@"native_space":@(sid),@"native_fullscreen":(spaceType&&sid&&spaceType(connection(),sid)!=0)?@YES:@NO,@"primary":CGDisplayIsMain(did)?@YES:@NO}];
            CFRelease(uuidString);CFRelease(uuid);
        }
    } else {
        NSMutableDictionary<NSNumber *,NSString *> *bundles=[NSMutableDictionary dictionary];
        CFArrayRef windows=CGWindowListCopyWindowInfo(kCGWindowListOptionAll|kCGWindowListExcludeDesktopElements,kCGNullWindowID);
        for (NSDictionary *w in (__bridge NSArray *)windows) {
            CGRect b; if (!CGRectMakeWithDictionaryRepresentation((__bridge CFDictionaryRef)w[(id)kCGWindowBounds],&b)) continue;
            uint32_t wid=[w[(id)kCGWindowNumber] unsignedIntValue];
            NSNumber *owner=w[(id)kCGWindowOwnerPID]?:@0;
            NSString *bundle=bundles[owner];
            if(!bundle) {
                bundle=[NSRunningApplication runningApplicationWithProcessIdentifier:owner.intValue].bundleIdentifier?:@"";
                bundles[owner]=bundle;
            }
            CFArrayRef spaces=connection&&windowSpaces?windowSpaces(connection(),7,(__bridge CFArrayRef)@[@(wid)]):NULL;
            bool sticky=false;
            bool stickyKnown=hasStickyAPI&&connection&&readSticky(&stickyAPI,connection(),wid,&sticky);
            [rows addObject:@{@"id":@(wid),@"pid":w[(id)kCGWindowOwnerPID]?:@0,@"app":w[(id)kCGWindowOwnerName]?:@"",
                @"title":w[(id)kCGWindowName]?:@"",@"layer":w[(id)kCGWindowLayer]?:@0,@"onscreen":[w[(id)kCGWindowIsOnscreen] boolValue]?@YES:@NO,
                @"bundle_id":bundle,
                @"bounds":rectJSON(b),@"native_spaces":spaces?(__bridge NSArray *)spaces:@[],
                @"sticky":@(sticky),@"sticky_known":@(stickyKnown)}];
            if(spaces)CFRelease(spaces);
        }
        if(windows)CFRelease(windows);
    }
    NSData *json=[NSJSONSerialization dataWithJSONObject:rows options:0 error:nil];
    char *result=malloc(json.length+1);if(!result)return NULL;memcpy(result,json.bytes,json.length);result[json.length]=0;return result;
} }
void ribbon_free(void *pointer) {free(pointer);}
int ribbon_ax_trusted(void) {return AXIsProcessTrusted();}
int ribbon_ax_request_permission(void) { @autoreleasepool {
    NSDictionary *options=@{(__bridge NSString *)kAXTrustedCheckOptionPrompt:@YES};
    return AXIsProcessTrustedWithOptions((__bridge CFDictionaryRef)options);
} }
static AXUIElementRef findAXWindow(uint32_t wid,int expected_pid,pid_t *pid) {
    resolve();if(!axWindowId)return NULL;
    // IncludingWindow alone omits windows on an inactive native Space. Resolve
    // the exact ID from the all-Spaces inventory so release can restore them.
    CFArrayRef list=CGWindowListCopyWindowInfo(kCGWindowListOptionAll|kCGWindowListExcludeDesktopElements,kCGNullWindowID);
    if(!list)return NULL;
    for(NSDictionary *info in (__bridge NSArray *)list) {
        if([info[(id)kCGWindowNumber] unsignedIntValue]==wid) {*pid=[info[(id)kCGWindowOwnerPID] intValue];break;}
    }
    CFRelease(list);
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
static int settleWindow(uint32_t wid,RibbonRect rect,BOOL exactPosition) {
    // AX replies can precede the owner's WindowServer move transaction. That
    // transaction translates the current transform relatively: resetting the
    // compositor before it arrives would apply the original offset twice.
    SkyLight sky;if(!loadSkyLight(&sky))return kAXErrorFailure;
    double deadline=NSProcessInfo.processInfo.systemUptime+0.5,stable=0;
    CGAffineTransform previous={0};BOOL havePrevious=NO;
    do {
        CGRect bounds;CGAffineTransform transform;
        CGError eb=sky.getBounds(sky.connection(),wid,&bounds),et=sky.getTransform(sky.connection(),wid,&transform);
        if(eb||et)return kAXErrorCannotComplete;
        BOOL position=fabs(bounds.origin.x-rect.x)<=2&&fabs(bounds.origin.y-rect.y)<=2;
        if(!exactPosition) {
            // AppKit may constrain the logical anchor. Visual placement belongs
            // to Dock, so accept a stable anchor inside the intended display.
            for(NSScreen *screen in NSScreen.screens) {
                CGRect display=CGDisplayBounds([screen.deviceDescription[@"NSScreenNumber"] unsignedIntValue]);
                if(CGRectContainsPoint(display,CGPointMake(rect.x,rect.y))) {
                    position=CGRectContainsPoint(display,bounds.origin);break;
                }
            }
        }
        BOOL matches=position&&fabs(bounds.size.width-rect.width)<=2&&fabs(bounds.size.height-rect.height)<=2;
        double now=NSProcessInfo.processInfo.systemUptime;
        if(matches&&havePrevious&&CGAffineTransformEqualToTransform(transform,previous)) {
            if(!stable)stable=now;
            if(now-stable>=0.05)return 0;
        } else stable=0;
        previous=transform;havePrevious=YES;usleep(5000);
    } while(NSProcessInfo.processInfo.systemUptime<deadline);
    CGRect finalBounds=CGRectZero;sky.getBounds(sky.connection(),wid,&finalBounds);
    fprintf(stderr,"Window %u did not settle: requested=(%.1f,%.1f,%.1f,%.1f), native=(%.1f,%.1f,%.1f,%.1f)\n",
        wid,rect.x,rect.y,rect.width,rect.height,finalBounds.origin.x,finalBounds.origin.y,finalBounds.size.width,finalBounds.size.height);
    return kAXErrorCannotComplete;
}
static AXError acceptedSize(AXUIElementRef window,CGSize expected,CGSize *accepted) {
    double deadline=NSProcessInfo.processInfo.systemUptime+0.25;
    for(;;) {
        CFTypeRef actual=NULL;AXError error=AXUIElementCopyAttributeValue(window,kAXSizeAttribute,&actual);
        if(!error&&(!actual||!AXValueGetValue(actual,kAXValueCGSizeType,accepted)))error=kAXErrorFailure;
        if(actual)CFRelease(actual);
        if(error)return error;
        if(fabs(accepted->width-expected.width)<=2&&fabs(accepted->height-expected.height)<=2)return 0;
        if(NSProcessInfo.processInfo.systemUptime>=deadline)return kAXErrorCannotComplete;
        usleep(5000);
    }
}
int ribbon_resize_window(uint32_t wid,int expected_pid,RibbonRect rect) { @autoreleasepool {
    pid_t pid=0;AXUIElementRef w=findAXWindow(wid,expected_pid,&pid);if(!w)return kAXErrorInvalidUIElement;
    // AppKit constrains a resize against the logical frame's current position.
    // Anchor it inside its monitor before sizing; the compositor owns the visual position.
    CGPoint position=CGPointMake(rect.x,rect.y);AXValueRef p=AXValueCreate(kAXValueCGPointType,&position);
    AXError error=AXUIElementSetAttributeValue(w,kAXPositionAttribute,p);CFRelease(p);
    CGSize size=CGSizeMake(rect.width,rect.height);AXValueRef value=AXValueCreate(kAXValueCGSizeType,&size);
    if(!error)error=AXUIElementSetAttributeValue(w,kAXSizeAttribute,value);CFRelease(value);
    CGSize accepted=CGSizeZero;
    if(!error)error=acceptedSize(w,size,&accepted);
    double dw=fabs(accepted.width-size.width),dh=fabs(accepted.height-size.height);
    if(error==kAXErrorCannotComplete&&accepted.width>=100&&accepted.height>=100&&dw<=32&&dh<=32&&(dw>2||dh>2)) {
        // Some owners ignore small changes at a maximized edge. Make a bounded
        // inward resize first, then request the exact size from a valid anchor.
        // This is not size negotiation: the final size must still match.
        CGSize inward=CGSizeMake(dw>2?MAX(100,size.width-40):size.width,dh>2?MAX(100,size.height-40):size.height);
        value=AXValueCreate(kAXValueCGSizeType,&inward);
        error=AXUIElementSetAttributeValue(w,kAXSizeAttribute,value);CFRelease(value);
        if(!error)error=acceptedSize(w,inward,&accepted);
        p=AXValueCreate(kAXValueCGPointType,&position);
        if(!error)error=AXUIElementSetAttributeValue(w,kAXPositionAttribute,p);CFRelease(p);
        value=AXValueCreate(kAXValueCGSizeType,&size);
        if(!error)error=AXUIElementSetAttributeValue(w,kAXSizeAttribute,value);CFRelease(value);
        if(!error)error=acceptedSize(w,size,&accepted);
    }
    // Position writes can overwrite an owner-side pending resize with its old
    // frame. Re-anchor only after the new size has actually been accepted.
    p=AXValueCreate(kAXValueCGPointType,&position);
    if(!error)error=AXUIElementSetAttributeValue(w,kAXPositionAttribute,p);CFRelease(p);
    if(error)fprintf(stderr,"Window %u AX resize failed (%d): requested=(%.1f,%.1f), accepted=(%.1f,%.1f)\n",wid,error,size.width,size.height,accepted.width,accepted.height);
    CFRelease(w);return error?error:settleWindow(wid,rect,NO);
} }
int ribbon_restore_window(uint32_t wid,int expected_pid,RibbonRect rect) { @autoreleasepool {
    int resized=ribbon_resize_window(wid,expected_pid,rect);if(resized)return resized;
    pid_t pid=0;AXUIElementRef w=findAXWindow(wid,expected_pid,&pid);if(!w)return kAXErrorInvalidUIElement;
    // Reapply the exact original position only after the size has settled, so
    // AppKit doesn't constrain it against the larger managed size.
    CGPoint point=CGPointMake(rect.x,rect.y);AXValueRef p=AXValueCreate(kAXValueCGPointType,&point);
    AXError ep=AXUIElementSetAttributeValue(w,kAXPositionAttribute,p);
    CFRelease(p);CFRelease(w);if(ep)return ep;
    return settleWindow(wid,rect,YES);
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
    CFRunLoopRunInMode(kCFRunLoopDefaultMode,0.001,false);
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
} }
void ribbon_pointer(double *x,double *y) {
    CGEventRef e=CGEventCreate(NULL);CGPoint p=CGEventGetLocation(e);*x=p.x;*y=p.y;CFRelease(e);
}
int ribbon_left_mouse_down(void) {
    return CGEventSourceButtonState(kCGEventSourceStateCombinedSessionState,kCGMouseButtonLeft);
}
