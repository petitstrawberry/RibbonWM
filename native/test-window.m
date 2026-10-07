// Independent AppKit fixture. Normal mode leaves transforms to the controller.
// Geometry regression mode changes only its own window, without Dock injection.
#import "skylight.h"
#include "bridge.h"
#include <libproc.h>

static void reply(NSDictionary *value) {
    NSData *data=[NSJSONSerialization dataWithJSONObject:value options:0 error:nil];
    fwrite(data.bytes,1,data.length,stdout);putchar('\n');fflush(stdout);
}
static id finiteNumber(double value) {return isfinite(value)?@(value):NSNull.null;}
static NSDictionary *surfaceState(SkyLight sky,uint32_t wid,BOOL includeOrder) {
    CGAffineTransform t={0};CGRect b=CGRectZero;
    CGError et=sky.getTransform(sky.connection(),wid,&t),eb=sky.getBounds(sky.connection(),wid,&b);
    CFTypeRef region=NULL;CGRect clip=CGRectZero;
    CGError (*regionBounds)(CFTypeRef,CGRect *)=dlsym(RTLD_DEFAULT,"CGSGetRegionBounds");
    CGError ec=sky.copyClip(sky.connection(),wid,&region);
    if(!ec&&region&&regionBounds)ec=regionBounds(region,&clip);
    if(region)sky.releaseRegion(region);
    NSMutableArray *order=[NSMutableArray array];
    CFArrayRef list=includeOrder?CGWindowListCopyWindowInfo(kCGWindowListOptionAll|kCGWindowListExcludeDesktopElements,kCGNullWindowID):NULL;
    for(NSDictionary *row in (__bridge NSArray *)list)if([row[(id)kCGWindowOwnerPID] intValue]==getpid())
        [order addObject:@{@"wid":row[(id)kCGWindowNumber],@"onscreen":@([row[(id)kCGWindowIsOnscreen] boolValue])}];
    if(list)CFRelease(list);
    return @{@"event":@"state",@"errors":@[@(et),@(eb)],@"pid":@(ribbon_window_owner(wid)),@"transform":@[finiteNumber(t.a),finiteNumber(t.b),finiteNumber(t.c),finiteNumber(t.d),finiteNumber(t.tx),finiteNumber(t.ty)],
        @"order":order,
        @"clip_error":@(ec),@"clip_bounds":@[finiteNumber(clip.origin.x),finiteNumber(clip.origin.y),finiteNumber(clip.size.width),finiteNumber(clip.size.height)],
        @"frame":@{@"x":finiteNumber(b.origin.x),@"y":finiteNumber(b.origin.y),@"width":finiteNumber(b.size.width),@"height":finiteNumber(b.size.height)}};
}
static int observeFixtureResize(void *context) {
    unsigned *count=context;(*count)++;return 0;
}
static NSDictionary *windowState(SkyLight sky,uint32_t wid) {return surfaceState(sky,wid,YES);}
static NSDictionary *associatedState(SkyLight sky,uint32_t wid) {
    void *h=dlopen("/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight",RTLD_NOW);
    CFArrayRef (*associated)(int,uint32_t)=dlsym(h,"SLSCopyAssociatedWindows");
    CFTypeRef (*query)(int,CFArrayRef,int)=dlsym(h,"SLSWindowQueryWindows");
    CFTypeRef (*iterator)(CFTypeRef)=dlsym(h,"SLSWindowQueryResultCopyWindows");
    bool (*advance)(CFTypeRef)=dlsym(h,"SLSWindowIteratorAdvance");
    uint32_t (*getID)(CFTypeRef)=dlsym(h,"SLSWindowIteratorGetWindowID");
    uint32_t (*parent)(CFTypeRef)=dlsym(h,"SLSWindowIteratorGetParentID");
    if(!associated||!query||!iterator||!advance||!getID||!parent)return @{@"unavailable":@YES};
    CFArrayRef ids=associated(sky.connection(),wid);
    NSMutableArray *rows=[NSMutableArray array];
    if(ids) {
        CFTypeRef q=query(sky.connection(),ids,(int)CFArrayGetCount(ids));
        CFTypeRef i=q?iterator(q):NULL;
        while(i&&advance(i)) {
            uint32_t id=getID(i);
            NSMutableDictionary *row=[windowState(sky,id) mutableCopy];
            row[@"wid"]=@(id);row[@"parent"]=@(parent(i));row[@"pid"]=@(ribbon_window_owner(id));
            [rows addObject:row];
        }
        if(i)CFRelease(i);if(q)CFRelease(q);CFRelease(ids);
    }
    return @{@"wid":@(wid),@"associated":rows};
}
@interface TestPanel : NSPanel
@end
@implementation TestPanel
- (BOOL)canBecomeKeyWindow {return NO;}
@end
@interface GeometryWindow : NSWindow
@property double refusedWidth;
@end
@implementation GeometryWindow
- (BOOL)canBecomeKeyWindow {return YES;}
- (void)setFrame:(NSRect)frame display:(BOOL)display {
    if(self.refusedWidth>0&&fabs(frame.size.width-self.refusedWidth)<2)frame.size.width+=80;
    [super setFrame:frame display:display];
}
@end
@interface TestView : NSView
@property BOOL target;
@property NSString *surface;
@end
@implementation TestView
- (BOOL)acceptsFirstMouse:(NSEvent *)event {(void)event;return YES;}
- (BOOL)shouldDelayWindowOrderingForEvent:(NSEvent *)event {(void)event;return YES;}
- (void)drawRect:(NSRect)dirty {
    (void)dirty;
    [(self.target?NSColor.systemBlueColor:[NSColor colorWithWhite:0.08 alpha:1]) setFill];NSRectFill(self.bounds);
    if(self.target)[@"External app window" drawAtPoint:NSMakePoint(20,320) withAttributes:@{
        NSFontAttributeName:[NSFont systemFontOfSize:24],NSForegroundColorAttributeName:NSColor.whiteColor}];
}
- (void)mouseDown:(NSEvent *)event {
    NSPoint p=[self convertPoint:event.locationInWindow fromView:nil];
    reply(@{@"event":@"click",@"target":self.target?@YES:@NO,@"surface":self.surface?:@"backdrop",@"x":@(p.x),@"y":@(p.y)});
}
@end
@interface Fixture : NSObject <NSApplicationDelegate>
@property NSWindow *target;
@property NSMutableArray<NSWindow *> *backdrops;
@property NSDictionary *configuration;
@property SkyLight sky;
@property NSRunningApplication *previous;
@end
@implementation Fixture
- (void)anchorNative {
    if(ribbon_left_mouse_down()||self.target.alphaValue==0)return;
    uint32_t wid=(uint32_t)self.target.windowNumber;CGRect bounds;CGAffineTransform t;
    if(self.sky.getBounds(self.sky.connection(),wid,&bounds)||self.sky.getTransform(self.sky.connection(),wid,&t))return;
    if(t.a!=1||t.d!=1||t.b!=0||t.c!=0)return;
    CGRect shown=CGRectMake(-t.tx,-t.ty,bounds.size.width,bounds.size.height);
    BOOL visible=NO;
    for(NSScreen *screen in NSScreen.screens) {
        CGRect display=CGDisplayBounds([screen.deviceDescription[@"NSScreenNumber"] unsignedIntValue]);
        if(CGRectContainsRect(display,shown))visible=YES;
    }
    if(!visible||(fabs(bounds.origin.x-shown.origin.x)<=1&&fabs(bounds.origin.y-shown.origin.y)<=1))return;
    if(self.sky.disableUpdates(self.sky.connection()))return;
    CGPoint point=shown.origin;
    CGError error=self.sky.moveWithGroup(self.sky.connection(),wid,&point);
    if(!error)error=self.sky.setTransform(self.sky.connection(),wid,t);
    self.sky.enableUpdates(self.sky.connection());
    reply(@{@"event":@"native-anchor",@"error":@(error),@"x":@(point.x),@"y":@(point.y)});
}
- (void)stop {
    [self.target close];for(NSWindow *panel in self.backdrops)[panel close];
    if(NSApp.active)[self.previous activateWithOptions:0];
    [NSApp stop:nil];[NSApp postEvent:[NSEvent otherEventWithType:NSEventTypeApplicationDefined
        location:NSZeroPoint modifierFlags:0 timestamp:0 windowNumber:0 context:nil subtype:0 data1:0 data2:0] atStart:NO];
}
- (void)command:(NSDictionary *)command {
    NSString *op=command[@"op"];
    if([op isEqual:@"refuse-width"]&&[self.configuration[@"geometry_test"] boolValue]) {
        double width=[command[@"width"] doubleValue];
        if(!isfinite(width)||width<100||width>2000)return;
        ((GeometryWindow *)self.target).refusedWidth=width;reply(@{@"refusing":@(width)});return;
    }
    if([op isEqual:@"quit"]) {[self stop];return;}
    if([op isEqual:@"present"]) {self.target.alphaValue=1;self.target.ignoresMouseEvents=NO;reply(@{@"presented":@YES});return;}
    if([op isEqual:@"anchor-now"]&&[self.configuration[@"geometry_test"] boolValue]) {[self anchorNative];return;}
    if([op isEqual:@"state"]) {
        NSMutableDictionary *state=[windowState(self.sky,(uint32_t)self.target.windowNumber) mutableCopy];
        state[@"alpha"]=@(self.target.alphaValue);state[@"visible"]=@(self.target.visible);
        state[@"ignores_mouse"]=@(self.target.ignoresMouseEvents);state[@"occlusion"]=@(self.target.occlusionState);
        reply(state);return;
    }
    if([op isEqual:@"translate"]&&[self.configuration[@"geometry_test"] boolValue]) {
        double x=[command[@"x"] doubleValue],y=[command[@"y"] doubleValue];
        if(!isfinite(x)||!isfinite(y)||fabs(x)>10000||fabs(y)>10000)return;
        CGError error=self.sky.setTransform(self.sky.connection(),(uint32_t)self.target.windowNumber,CGAffineTransformMakeTranslation(-x,-y));
        reply(@{@"transform_error":@(error)});return;
    }
    if([op isEqual:@"resize"]&&[self.configuration[@"geometry_test"] boolValue]) {
        double width=[command[@"width"] doubleValue],height=[command[@"height"] doubleValue];
        if(!isfinite(width)||!isfinite(height)||width<100||height<100||width>2000||height>1500)return;
        NSRect frame=self.target.frame;frame.origin.y+=frame.size.height-height;frame.size=NSMakeSize(width,height);
        [self.target setFrame:frame display:YES];reply(@{@"resized":@YES});return;
    }
    if([op isEqual:@"new"]&&[self.configuration[@"geometry_test"] boolValue]) {
        NSWindow *panel=[[GeometryWindow alloc] initWithContentRect:NSMakeRect(600,200,400,400)
            styleMask:NSWindowStyleMaskTitled|NSWindowStyleMaskResizable
            backing:NSBackingStoreBuffered defer:NO];
        panel.title=@"RibbonWM QA created";panel.hidesOnDeactivate=NO;
        panel.releasedWhenClosed=NO;panel.animationBehavior=NSWindowAnimationBehaviorNone;
        panel.collectionBehavior=NSWindowCollectionBehaviorMoveToActiveSpace;
        [panel orderFrontRegardless];[self.backdrops addObject:panel];
        reply(@{@"created":@(panel.windowNumber)});return;
    }
    if([op isEqual:@"close-new"]&&[self.configuration[@"geometry_test"] boolValue]) {
        for(NSWindow *window in self.backdrops)[window close];
        [self.backdrops removeAllObjects];reply(@{@"closed_new":@YES});return;
    }
    if([op isEqual:@"click"]||[op isEqual:@"move"]) {
        CGPoint p=CGPointMake([command[@"x"] doubleValue],[command[@"y"] doubleValue]);
        CGEventRef e=CGEventCreateMouseEvent(NULL,kCGEventMouseMoved,p,kCGMouseButtonLeft);CGEventPost(kCGHIDEventTap,e);CFRelease(e);
        if([op isEqual:@"move"]){reply(@{@"moved":@YES});return;}
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW,30*NSEC_PER_MSEC),dispatch_get_main_queue(),^{
            CGEventRef down=CGEventCreateMouseEvent(NULL,kCGEventLeftMouseDown,p,kCGMouseButtonLeft);CGEventPost(kCGHIDEventTap,down);CFRelease(down);
            dispatch_after(dispatch_time(DISPATCH_TIME_NOW,30*NSEC_PER_MSEC),dispatch_get_main_queue(),^{
                CGEventRef up=CGEventCreateMouseEvent(NULL,kCGEventLeftMouseUp,p,kCGMouseButtonLeft);CGEventPost(kCGHIDEventTap,up);CFRelease(up);
            });
        });
    }
}
- (void)applicationDidFinishLaunching:(NSNotification *)notification {
    (void)notification;
    if(!loadSkyLight(&_sky)){[self stop];return;}
    self.previous=NSWorkspace.sharedWorkspace.frontmostApplication;
    CGFloat height=NSScreen.screens[0].frame.size.height;
    if(NSScreen.mainScreen.frame.size.width<1400||height<820){reply(@{@"error":@"Fixture requires a 1400 x 820 point primary screen"});[self stop];return;}
    self.backdrops=[NSMutableArray array];
    NSArray *surfaces=self.configuration[@"backdrops"]?:@[@{@"x":@780,@"y":@320,@"width":@600,@"height":@480,@"surface":@"backdrop"}];
    for(NSDictionary *surface in surfaces) {
        double x=[surface[@"x"] doubleValue],y=[surface[@"y"] doubleValue],w=[surface[@"width"] doubleValue],h=[surface[@"height"] doubleValue];
        TestPanel *panel=[[TestPanel alloc] initWithContentRect:NSMakeRect(x,height-y-h,w,h) styleMask:NSWindowStyleMaskBorderless|NSWindowStyleMaskNonactivatingPanel backing:NSBackingStoreBuffered defer:NO];
        panel.level=NSNormalWindowLevel;panel.hidesOnDeactivate=NO;panel.hasShadow=NO;
        panel.collectionBehavior=NSWindowCollectionBehaviorMoveToActiveSpace;
        TestView *background=[[TestView alloc] initWithFrame:NSMakeRect(0,0,w,h)];background.surface=surface[@"surface"];panel.contentView=background;
        [panel orderFrontRegardless];[self.backdrops addObject:panel];
    }
    NSDictionary *origin=self.configuration[@"target"]?:@{@"x":@940,@"y":@360};
    NSUInteger style=[self.configuration[@"geometry_test"] boolValue]?
        NSWindowStyleMaskTitled|NSWindowStyleMaskResizable:
        NSWindowStyleMaskBorderless|NSWindowStyleMaskNonactivatingPanel;
    Class kind=[self.configuration[@"geometry_test"] boolValue]?GeometryWindow.class:TestPanel.class;
    self.target=[[kind alloc] initWithContentRect:NSMakeRect([origin[@"x"] doubleValue],height-[origin[@"y"] doubleValue]-400,400,400) styleMask:style backing:NSBackingStoreBuffered defer:NO];
    self.target.title=@"RibbonWM QA geometry";
    self.target.releasedWhenClosed=NO;self.target.animationBehavior=NSWindowAnimationBehaviorNone;
    self.target.level=[self.configuration[@"geometry_test"] boolValue]?NSNormalWindowLevel:NSFloatingWindowLevel;
    self.target.hidesOnDeactivate=NO;self.target.hasShadow=[self.configuration[@"shadow"] boolValue];
    self.target.collectionBehavior=NSWindowCollectionBehaviorMoveToActiveSpace;
    self.target.alphaValue=0;self.target.ignoresMouseEvents=YES;
    TestView *view=[[TestView alloc] initWithFrame:NSMakeRect(0,0,400,400)];view.target=YES;view.surface=@"target";self.target.contentView=view;
    [self.target orderFrontRegardless];[self.target displayIfNeeded];
    if([self.configuration[@"anchor_idle"] boolValue])
        [NSTimer scheduledTimerWithTimeInterval:0.016 target:self selector:@selector(anchorNative) userInfo:nil repeats:YES];

    dispatch_after(dispatch_time(DISPATCH_TIME_NOW,200*NSEC_PER_MSEC),dispatch_get_main_queue(),^{
        reply(@{@"ready":@YES,@"pid":@(getpid()),@"wid":@(self.target.windowNumber)});
        dispatch_async(dispatch_get_global_queue(QOS_CLASS_USER_INITIATED,0),^{
            char *line=NULL;size_t capacity=0;
            while(getline(&line,&capacity,stdin)>0) {
                NSData *data=[[NSString stringWithUTF8String:line] dataUsingEncoding:NSUTF8StringEncoding];
                NSDictionary *command=[NSJSONSerialization JSONObjectWithData:data options:0 error:nil];
                dispatch_async(dispatch_get_main_queue(),^{[self command:command];});
            }
            free(line);dispatch_async(dispatch_get_main_queue(),^{[self stop];});
        });
    });
    // The fixture owns no persistent surface, even if its controller crashes.
    double lifetime=self.configuration[@"lifetime"]?fmin(120,fmax(1,[self.configuration[@"lifetime"] doubleValue])):30;
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW,(int64_t)(lifetime*NSEC_PER_SEC)),dispatch_get_main_queue(),^{[self stop];});
}
@end
int main(int argc,char **argv) {@autoreleasepool {
    if(argc==2&&!strcmp(argv[1],"--session-state")) {
        printf("{\"session_active\":%s}\n",ribbon_session_active()?"true":"false");return 0;
    }
    if(argc==3&&!strcmp(argv[1],"--associated")) {
        uint32_t wid=(uint32_t)strtoul(argv[2],NULL,10);SkyLight sky;
        if(!wid||!loadSkyLight(&sky))return 1;
        reply(associatedState(sky,wid));return 0;
    }
    if(argc==5&&!strcmp(argv[1],"--trace-pair")) {
        id ids=[NSJSONSerialization JSONObjectWithData:[[NSString stringWithUTF8String:argv[2]] dataUsingEncoding:NSUTF8StringEncoding] options:0 error:nil];
        int pid=atoi(argv[3]);double seconds=strtod(argv[4],NULL);
        char own[PROC_PIDPATHINFO_MAXSIZE]={0},other[PROC_PIDPATHINFO_MAXSIZE]={0};
        if(![ids isKindOfClass:NSArray.class]||[ids count]!=2||pid<=0||!isfinite(seconds)||seconds<=0||seconds>30||
            proc_pidpath(getpid(),own,sizeof(own))<=0||proc_pidpath(pid,other,sizeof(other))<=0||strcmp(own,other))return 1;
        for(id value in ids)if(![value isKindOfClass:NSNumber.class]||![value unsignedIntValue]||ribbon_window_owner([value unsignedIntValue])!=pid)return 1;
        SkyLight sky;if(!loadSkyLight(&sky))return 1;double until=ribbon_input_time()+seconds;
        while(ribbon_input_time()<until) {@autoreleasepool {
            NSMutableArray *rows=[NSMutableArray array];
            for(NSNumber *value in ids) {
                uint32_t wid=value.unsignedIntValue;if(ribbon_window_owner(wid)!=pid)return 1;
                NSMutableDictionary *row=[surfaceState(sky,wid,NO) mutableCopy];row[@"wid"]=value;[rows addObject:row];
            }
            // Read both surfaces again, in reverse order. A mismatch identifies
            // a sample that crossed an owner resize or compositor publication.
            BOOL coherent=YES;
            for(NSUInteger index=rows.count;index>0;index--) {
                NSDictionary *first=rows[index-1];uint32_t wid=[first[@"wid"] unsignedIntValue];
                if(ribbon_window_owner(wid)!=pid)return 1;
                NSMutableDictionary *again=[surfaceState(sky,wid,NO) mutableCopy];again[@"wid"]=first[@"wid"];
                if(![again isEqual:first])coherent=NO;
            }
            reply(@{@"event":@"frame-sample",@"time":@(ribbon_input_time()),@"coherent":@(coherent),@"windows":rows});usleep(4000);
        }}return 0;
    }
    if(argc==5&&!strcmp(argv[1],"--trace-window")) {
        // Diagnostic only: observe the exact owner without AX, input synthesis,
        // compositor writes, enrollment, or changing the production service.
        uint32_t wid=(uint32_t)strtoul(argv[2],NULL,10);int pid=atoi(argv[3]);
        double seconds=strtod(argv[4],NULL);
        NSRunningApplication *app=[NSRunningApplication runningApplicationWithProcessIdentifier:pid];
        if(!wid||pid<=0||!app||ribbon_window_owner(wid)!=pid||
            [app.bundleIdentifier hasPrefix:@"com.openai."]||[app.localizedName hasPrefix:@"ChatGPT"]||
            [app.localizedName hasPrefix:@"Codex"]||[app.bundleIdentifier isEqual:@"com.apple.systempreferences"]||
            !isfinite(seconds)||seconds<=0||seconds>180)return 1;
        SkyLight sky;if(!loadSkyLight(&sky))return 1;
        double until=ribbon_input_time()+seconds;
        reply(@{@"tracing":@YES,@"wid":@(wid),@"pid":@(pid)});
        while(ribbon_input_time()<until) {@autoreleasepool {
            if(ribbon_window_owner(wid)!=pid)return 1;
            CFRunLoopRunInMode(kCFRunLoopDefaultMode,0,true);
            RibbonMouseState mouse=ribbon_mouse_state();double x=0,y=0;ribbon_pointer(&x,&y);
            NSMutableDictionary *state=[windowState(sky,wid) mutableCopy];
            state[@"time"]=@(ribbon_input_time());
            state[@"mouse"]=@{@"down":@(ribbon_left_mouse_down()),@"dragged":@(mouse.dragged),
                @"press_x":@(mouse.x),@"press_y":@(mouse.y),@"window":@(mouse.window),@"x":@(x),@"y":@(y)};
            reply(state);usleep(2000);
        }}
        return 0;
    }
    if(argc==3&&!strcmp(argv[1],"--overview-trace")) {
        double seconds=strtod(argv[2],NULL);
        if(!isfinite(seconds)||seconds<=0||seconds>180)return 1;
        double until=ribbon_input_time()+seconds;int previous=-1;
        while(ribbon_input_time()<until) {@autoreleasepool {
            CFRunLoopRunInMode(kCFRunLoopDefaultMode,0,true);
            int active=ribbon_mission_control_active();
            if(active!=previous)reply(@{@"time":@(ribbon_input_time()),@"active":@(active)});
            previous=active;usleep(4000);
        }}
        return 0;
    }
    if(argc==2&&!strcmp(argv[1],"--overview-state")) {reply(@{@"active":@(ribbon_mission_control_active())});return 0;}
    if(argc==2&&!strcmp(argv[1],"--overview-exit")) {
        if(!ribbon_mission_control_active())return 1;
        CGEventRef down=CGEventCreateKeyboardEvent(NULL,53,true),up=CGEventCreateKeyboardEvent(NULL,53,false);
        CGEventPost(kCGHIDEventTap,down);CGEventPost(kCGHIDEventTap,up);CFRelease(down);CFRelease(up);return 0;
    }
    if((argc==2||argc==3)&&!strcmp(argv[1],"--permission-only")) {
        NSMutableDictionary *state=[@{@"cached":@(AXIsProcessTrusted()),@"live":@(ribbon_ax_trusted()),@"pid":@(getpid())} mutableCopy];
        if(argc==3)state[@"owned_access"]=@(ribbon_owned_probe_accessible(atoi(argv[2])));
        reply(state);return 0;
    }
    if(argc==5&&!strcmp(argv[1],"--sample-surface")) {
        uint32_t wid=(uint32_t)strtoul(argv[2],NULL,10);int pid=atoi(argv[3]);
        double seconds=strtod(argv[4],NULL);if(!wid||pid<=0||!isfinite(seconds)||seconds<=0||seconds>10)return 1;
        BOOL own=NO;CFArrayRef list=CGWindowListCopyWindowInfo(kCGWindowListOptionAll,kCGNullWindowID);
        for(NSDictionary *row in (__bridge NSArray *)list)if([row[(id)kCGWindowNumber] unsignedIntValue]==wid&&
            [row[(id)kCGWindowOwnerPID] intValue]==pid&&[row[(id)kCGWindowName] hasPrefix:@"RibbonWM QA "])own=YES;
        if(list)CFRelease(list);if(!own)return 1;
        SkyLight sky;if(!loadSkyLight(&sky))return 1;
        NSMutableArray *samples=[NSMutableArray array];double until=ribbon_input_time()+seconds;
        reply(@{@"sampling":@YES});
        while(ribbon_input_time()<until) {
            if(ribbon_window_owner(wid)!=pid)return 1;
            CGRect bounds=CGRectZero,clip=CGRectZero;CGAffineTransform transform={0};CFTypeRef region=NULL;
            CGError eb=sky.getBounds(sky.connection(),wid,&bounds),et=sky.getTransform(sky.connection(),wid,&transform);
            CGError ec=sky.copyClip(sky.connection(),wid,&region);
            CGError (*regionBounds)(CFTypeRef,CGRect *)=dlsym(RTLD_DEFAULT,"CGSGetRegionBounds");
            if(!ec&&region&&regionBounds)ec=regionBounds(region,&clip);
            if(region)sky.releaseRegion(region);
            [samples addObject:@{@"time":@(ribbon_input_time()),@"errors":@[@(et),@(eb)],
                @"transform":@[@(transform.a),@(transform.b),@(transform.c),@(transform.d),@(transform.tx),@(transform.ty)],
                @"clip_error":@(ec),@"clip_bounds":@[finiteNumber(clip.origin.x),finiteNumber(clip.origin.y),finiteNumber(clip.size.width),finiteNumber(clip.size.height)]}];usleep(2000);
        }
        reply(@{@"samples":samples});return 0;
    }
    if(argc==2&&!strcmp(argv[1],"--gesture-ready")) {
        ribbon_probe_show("実ウィンドウでOption＋2本指スクロールを試します\n\n「計測開始」を押すと、検証用端末5枚を\n並べて管理を開始します。\n他のアプリは管理しません。\n\n開始後は5分間試せます。\nボタンを押すまで待機します。");
        double until=ribbon_input_time()+600;
        while(ribbon_input_time()<until) {
            uint32_t action=ribbon_probe_action();
            if(action&2){ribbon_probe_close();return 2;}
            if(action&1){ribbon_probe_close();reply(@{@"start":@YES});return 0;}
            CFRunLoopRunInMode(kCFRunLoopDefaultMode,0.01,true);
        }
        ribbon_probe_close();return 2;
    }
    if(argc==4&&!strcmp(argv[1],"--read-geometry")) {
        uint32_t wid=(uint32_t)strtoul(argv[2],NULL,10);int pid=atoi(argv[3]);
        NSRunningApplication *app=[NSRunningApplication runningApplicationWithProcessIdentifier:pid];
        if(!wid||pid<=0||!app||[app.bundleIdentifier hasPrefix:@"com.openai."]||
            [app.localizedName hasPrefix:@"ChatGPT"]||[app.localizedName hasPrefix:@"Codex"]||
            [app.bundleIdentifier isEqual:@"com.apple.systempreferences"])return 1;
        RibbonRect rect={0};int error=ribbon_window_geometry(wid,pid,&rect);
        reply(@{@"geometry_error":@(error),@"geometry":@{@"x":@(rect.x),@"y":@(rect.y),@"width":@(rect.width),@"height":@(rect.height)}});return error?1:0;
    }
    if(argc==2&&!strcmp(argv[1],"--viewport-test")) {
        RibbonRect primary=ribbon_display_viewport((RibbonRect){0,0,1512,982},(RibbonRect){0,0,1512,949},(RibbonRect){0,0,1512,982},33);
        RibbonRect upper=ribbon_display_viewport((RibbonRect){-100,982,1600,900},(RibbonRect){-100,982,1600,900},(RibbonRect){-100,-900,1600,900},33);
        RibbonRect lower=ribbon_display_viewport((RibbonRect){700,-1000,1800,1000},(RibbonRect){710,-960,1780,960},(RibbonRect){700,982,1800,1000},33);
        if(primary.y!=33||primary.height!=949||upper.y!=-867||upper.height!=867||
            lower.x!=710||lower.y!=1015||lower.height!=927)return 1;
        RibbonRect converted=ribbon_outer_to_ax((RibbonRect){24,57,600,909},(RibbonRect){208,81,501,901},(RibbonRect){208,72,501,910});
        if(converted.x!=24||converted.y!=66||converted.width!=600||converted.height!=900)return 1;
        puts("PASS: menu bar reservation follows each CG display origin, including monitors above primary");return 0;
    }
    // External focus event for QA: only the explicitly named disposable
    // Alacritty window is eligible, never a user's ordinary terminal or Codex.
    if((argc==4&&(strcmp(argv[1],"--focus-fixture")==0||strcmp(argv[1],"--click-fixture")==0||strcmp(argv[1],"--geometry-fixture")==0))||
       (argc==6&&(strcmp(argv[1],"--resize-fixture")==0||strcmp(argv[1],"--drag-fixture")==0||strcmp(argv[1],"--move-drag-fixture")==0||strcmp(argv[1],"--scroll-fixture")==0))||
       (argc==7&&strcmp(argv[1],"--resize-calibrated-fixture")==0)||
       (argc==8&&strcmp(argv[1],"--restore-fixture")==0)) {
        BOOL click=strcmp(argv[1],"--click-fixture")==0;
        BOOL calibrated=strcmp(argv[1],"--resize-calibrated-fixture")==0;
        BOOL resize=strcmp(argv[1],"--resize-fixture")==0||calibrated;
        BOOL moveDrag=strcmp(argv[1],"--move-drag-fixture")==0;
        BOOL drag=strcmp(argv[1],"--drag-fixture")==0||moveDrag;
        BOOL scroll=strcmp(argv[1],"--scroll-fixture")==0;
        BOOL geometry=strcmp(argv[1],"--geometry-fixture")==0;
        BOOL restore=strcmp(argv[1],"--restore-fixture")==0;
        char *end=NULL;unsigned long wid=strtoul(argv[2],&end,10);
        if(!wid||wid>UINT32_MAX||*end)return 1;
        long pid=strtol(argv[3],&end,10);if(pid<=0||pid>INT_MAX||*end)return 1;
        NSRunningApplication *app=[NSRunningApplication runningApplicationWithProcessIdentifier:(pid_t)pid];
        // Nix's executable wrapper can launch Alacritty without a bundle ID.
        char ownPath[PROC_PIDPATHINFO_MAXSIZE]={0},targetPath[PROC_PIDPATHINFO_MAXSIZE]={0};
        BOOL ownFixture=proc_pidpath(getpid(),ownPath,sizeof(ownPath))>0&&
            proc_pidpath((pid_t)pid,targetPath,sizeof(targetPath))>0&&!strcmp(ownPath,targetPath);
        if(![app.bundleIdentifier isEqual:@"org.alacritty"]&&![app.localizedName.lowercaseString containsString:@"alacritty"]&&
            !(ownFixture&&(geometry||resize||restore))){reply(@{@"error":@"Not the fixture app",@"bundle":app.bundleIdentifier?:@"",@"app":app.localizedName?:@""});return 1;}
        CFArrayRef list=CGWindowListCopyWindowInfo(kCGWindowListOptionAll|kCGWindowListExcludeDesktopElements,kCGNullWindowID);
        BOOL fixture=NO;
        for(NSDictionary *row in (__bridge NSArray *)list) {
            if([row[(id)kCGWindowNumber] unsignedIntValue]==wid&&[row[(id)kCGWindowOwnerPID] intValue]==pid&&
                [row[(id)kCGWindowName] hasPrefix:@"RibbonWM QA "]&&
                (!(click||drag||scroll)||[row[(id)kCGWindowIsOnscreen] boolValue])) {fixture=YES;break;}
        }
        if(list)CFRelease(list);
        if(!fixture){reply(@{@"error":@"Not a named fixture window"});return 1;}
        if(geometry) {
            RibbonRect rect={0};int error=ribbon_window_geometry((uint32_t)wid,(int)pid,&rect);
            uint32_t candidate=(uint32_t)wid;char *raw=ribbon_probe_application((int)pid,&candidate,1);
            AXUIElementRef application=AXUIElementCreateApplication((pid_t)pid);CFTypeRef members=NULL;
            AXUIElementSetMessagingTimeout(application,0.25);
            AXError membershipError=AXUIElementCopyAttributeValue(application,kAXWindowsAttribute,&members);
            CFIndex memberCount=members&&CFGetTypeID(members)==CFArrayGetTypeID()?CFArrayGetCount(members):-1;
            NSMutableArray *memberIDs=[NSMutableArray array];
            CGError (*identifier)(AXUIElementRef,uint32_t *)=dlsym(RTLD_DEFAULT,"_AXUIElementGetWindow");
            if(memberCount>0)for(id member in (__bridge NSArray *)members) {
                uint32_t memberID=0;CGError error=identifier?identifier((__bridge AXUIElementRef)member,&memberID):-1;
                CFTypeRef role=NULL,title=NULL;
                AXError re=AXUIElementCopyAttributeValue((__bridge AXUIElementRef)member,kAXRoleAttribute,&role);
                AXError te=AXUIElementCopyAttributeValue((__bridge AXUIElementRef)member,kAXTitleAttribute,&title);
                [memberIDs addObject:@{@"id":@(memberID),@"error":@(error),@"role_error":@(re),@"title_error":@(te),@"role":role?(__bridge id)role:NSNull.null,@"title":title?(__bridge id)title:NSNull.null}];
                if(role)CFRelease(role);if(title)CFRelease(title);
            }
            if(members)CFRelease(members);CFRelease(application);
            id probe=raw?[NSJSONSerialization JSONObjectWithData:[[NSString stringWithUTF8String:raw] dataUsingEncoding:NSUTF8StringEncoding] options:0 error:nil]:NSNull.null;
            if(raw)ribbon_free(raw);
            reply(@{@"geometry_error":@(error),@"membership_error":@(membershipError),@"member_count":@(memberCount),@"members":memberIDs,@"probe":probe?:NSNull.null,@"manageable":@(ribbon_window_manageable((uint32_t)wid,(int)pid)),@"geometry":@{@"x":@(rect.x),@"y":@(rect.y),@"width":@(rect.width),@"height":@(rect.height)}});return error?1:0;
        }
        if(restore) {
            double values[4];for(int i=0;i<4;i++) {
                values[i]=strtod(argv[i+4],&end);if(*end||!isfinite(values[i])||fabs(values[i])>10000)return 1;
            }
            if(values[2]<100||values[3]<100)return 1;
            int error=ribbon_restore_window((uint32_t)wid,(int)pid,(RibbonRect){values[0],values[1],values[2],values[3]});
            reply(@{@"restore_error":@(error)});return error?1:0;
        }
        if(resize) {
            double width=strtod(argv[4],&end);if(*end||!isfinite(width)||width<100||width>10000)return 1;
            double height=strtod(argv[5],&end);if(*end||!isfinite(height)||height<100||height>10000)return 1;
            SkyLight sky;if(!loadSkyLight(&sky))return 1;CGRect frame;
            if(sky.getBounds(sky.connection(),(uint32_t)wid,&frame))return 1;
            int result=0;
            if(calibrated) {
                if(!ownFixture)return 1;
                id data=[NSJSONSerialization JSONObjectWithData:[[NSString stringWithUTF8String:argv[6]] dataUsingEncoding:NSUTF8StringEncoding] options:0 error:nil];
                if(![data isKindOfClass:NSArray.class]||[data count]!=8)return 1;
                double v[8];for(int i=0;i<8;i++) {
                    if(![data[i] isKindOfClass:NSNumber.class])return 1;
                    v[i]=[data[i] doubleValue];if(!isfinite(v[i])||fabs(v[i])>100000)return 1;
                }
                if(v[2]<=0||v[3]<=0||v[6]<=0||v[7]<=0)return 1;
                void *pending=NULL;unsigned progressCount=0;
                result=ribbon_resize_begin((uint32_t)wid,(int)pid,(RibbonRect){frame.origin.x,frame.origin.y,width,height},
                    (RibbonRect){v[0],v[1],v[2],v[3]},(RibbonRect){v[4],v[5],v[6],v[7]},observeFixtureResize,&progressCount,&pending);
                reply(@{@"resize_progress_callbacks":@(progressCount)});
                if(!result){do {result=ribbon_settlement_poll(pending);if(!result)usleep(5000);}while(!result);if(result==1)result=0;}
                if(pending)ribbon_settlement_release(pending);
            } else result=ribbon_resize_window((uint32_t)wid,(int)pid,(RibbonRect){frame.origin.x,frame.origin.y,width,height});
            reply(@{@"resize_error":@(result),@"state":windowState(sky,(uint32_t)wid)});return result?1:0;
        }
        if(click||drag||scroll) {
            SkyLight sky;if(!loadSkyLight(&sky))return 1;
            NSDictionary *state=windowState(sky,(uint32_t)wid);
            NSArray *t=state[@"transform"],*clip=state[@"clip_bounds"];
            if([state[@"errors"][0] intValue]||[state[@"errors"][1] intValue]||[state[@"clip_error"] intValue]||
                [t[0] doubleValue]!=1||[t[3] doubleValue]!=1||[t[1] doubleValue]!=0||[t[2] doubleValue]!=0||
                [clip[2] doubleValue]<40||[clip[3] doubleValue]<100||ribbon_window_owner((uint32_t)wid)!=pid)return 1;
            // Click the middle of this disposable window's visible content;
            // do not activate it first, so this exercises OS-driven focus.
            CGPoint point=CGPointMake(-[t[4] doubleValue]+[clip[0] doubleValue]+[clip[2] doubleValue]/2,
                                      -[t[5] doubleValue]+[clip[1] doubleValue]+[clip[3] doubleValue]/2);
            if(scroll) {
                long direct=strtol(argv[4],&end,10);if(*end||labs(direct)>1000)return 1;
                long tail=strtol(argv[5],&end,10);if(*end||labs(tail)>1000)return 1;
                if(ribbon_focused_window((int)pid)!=wid)return 1;
                CGEventRef move=CGEventCreateMouseEvent(NULL,kCGEventMouseMoved,point,kCGMouseButtonLeft);
                CGEventPost(kCGHIDEventTap,move);CFRelease(move);usleep(100000);
                int phases[]={kCGScrollPhaseBegan,kCGScrollPhaseEnded,0,0};
                int momenta[]={0,0,kCGMomentumScrollPhaseBegin,kCGMomentumScrollPhaseEnd};
                long deltas[]={direct,0,tail,0};
                for(int i=0;i<4;i++) {
                    CGEventRef event=CGEventCreateScrollWheelEvent(NULL,kCGScrollEventUnitPixel,2,0,(int32_t)deltas[i]);
                    CGEventSetLocation(event,point);CGEventSetFlags(event,i==0?kCGEventFlagMaskAlternate:0);
                    CGEventSetIntegerValueField(event,kCGScrollWheelEventIsContinuous,1);
                    CGEventSetIntegerValueField(event,kCGScrollWheelEventScrollPhase,phases[i]);
                    CGEventSetIntegerValueField(event,kCGScrollWheelEventMomentumPhase,momenta[i]);
                    CGEventPost(kCGHIDEventTap,event);CFRelease(event);usleep(80000);
                }
                reply(@{@"posted_scroll":@(direct),@"posted_momentum":@(tail)});return 0;
            }
            double dx=0,dy=0;
            if(drag) {
                dx=strtod(argv[4],&end);if(*end||!isfinite(dx)||fabs(dx)>200)return 1;
                dy=strtod(argv[5],&end);if(*end||!isfinite(dy)||fabs(dy)>200||(!moveDrag&&dy!=0))return 1;
                if(ribbon_focused_window((int)pid)!=wid||[clip[0] doubleValue]!=0||fabs([clip[2] doubleValue]-[state[@"frame"][@"width"] doubleValue])>1)return 1;
                point.x=-[t[4] doubleValue]+[clip[2] doubleValue]-1;
                if(moveDrag)point=CGPointMake(-[t[4] doubleValue]+[clip[2] doubleValue]/2,-[t[5] doubleValue]+10);
            }
            CGEventRef move=CGEventCreateMouseEvent(NULL,kCGEventMouseMoved,point,kCGMouseButtonLeft);
            CGEventRef down=CGEventCreateMouseEvent(NULL,kCGEventLeftMouseDown,point,kCGMouseButtonLeft);
            CGEventRef up=CGEventCreateMouseEvent(NULL,kCGEventLeftMouseUp,point,kCGMouseButtonLeft);
            CGEventPost(kCGHIDEventTap,move);usleep(100000);
            NSDictionary *beforeDown=moveDrag?windowState(sky,(uint32_t)wid):nil;
            CGEventPost(kCGHIDEventTap,down);
            NSMutableArray *pressSamples=[NSMutableArray array];
            for(int i=0;i<10;i++) {
                usleep(5000);
                if(moveDrag)[pressSamples addObject:windowState(sky,(uint32_t)wid)];
            }
            if(drag) {
                NSMutableArray *samples=[NSMutableArray array];
                for(unsigned i=1;i<=10;i++) {
                    CGPoint next=CGPointMake(point.x+dx*i/10,point.y+dy*i/10);
                    CGEventRef event=CGEventCreateMouseEvent(NULL,kCGEventLeftMouseDragged,next,kCGMouseButtonLeft);
                    CGEventPost(kCGHIDEventTap,event);CFRelease(event);usleep(50000);
                    if(moveDrag)[samples addObject:windowState(sky,(uint32_t)wid)];
                }
                CGEventSetLocation(up,CGPointMake(point.x+dx,point.y+dy));
                if(moveDrag)reply(@{@"drag_samples":samples,@"before_down":beforeDown,@"press_samples":pressSamples,
                    @"press":@[@(point.x),@(point.y)]});
            }
            CGEventPost(kCGHIDEventTap,up);
            CFRelease(move);CFRelease(down);CFRelease(up);
        } else {
            int focused=ribbon_focus_window((uint32_t)wid,(int)pid);
            if(focused){reply(@{@"error":@"AX focus failed",@"code":@(focused)});return 1;}
        }
        for(unsigned i=0;i<40;i++) {
            if(ribbon_focused_window((int)pid)==wid) {
                reply(@{@"focused":@(wid),@"pid":@(pid)});return 0;
            }
            usleep(10000);
        }
        AXUIElementRef application=AXUIElementCreateApplication((pid_t)pid);CFTypeRef frontmost=NULL;
        AXUIElementSetMessagingTimeout(application,0.05);
        AXError read=AXUIElementCopyAttributeValue(application,kAXFrontmostAttribute,&frontmost);
        reply(@{@"error":@"Native focus did not settle",@"frontmost_pid":@(ribbon_frontmost_pid()),@"focused_window":@(ribbon_focused_window((int)pid)),
                @"ax_frontmost":(!read&&frontmost)?(__bridge id)frontmost:NSNull.null});
        if(frontmost)CFRelease(frontmost);CFRelease(application);return 1;
    }
    if(argc==4&&strcmp(argv[1],"--compare-images")==0) {
        NSBitmapImageRep *a=[NSBitmapImageRep imageRepWithData:[NSData dataWithContentsOfFile:@(argv[2])]];
        NSBitmapImageRep *b=[NSBitmapImageRep imageRepWithData:[NSData dataWithContentsOfFile:@(argv[3])]];
        if(!a||!b||a.pixelsWide!=b.pixelsWide||a.pixelsHigh!=b.pixelsHigh)return 1;
        unsigned long changed=0;double maximum=0;
        for(NSInteger y=0;y<a.pixelsHigh;y++) {@autoreleasepool {
            for(NSInteger x=0;x<a.pixelsWide;x++) {
                NSColor *ca=[[a colorAtX:x y:y] colorUsingColorSpace:NSColorSpace.deviceRGBColorSpace];
                NSColor *cb=[[b colorAtX:x y:y] colorUsingColorSpace:NSColorSpace.deviceRGBColorSpace];
                double delta=fmax(fabs(ca.redComponent-cb.redComponent),fmax(fabs(ca.greenComponent-cb.greenComponent),fabs(ca.blueComponent-cb.blueComponent)));
                maximum=fmax(maximum,delta);if(delta>2.0/255)changed++;
            }
        }
        }
        reply(@{@"pixels":@(a.pixelsWide*a.pixelsHigh),@"changed":@(changed),@"max_difference":@(maximum)});return 0;
    }
    if(argc==2&&strcmp(argv[1],"--spaces")==0) {
        SkyLight sky;if(!loadSkyLight(&sky))return 1;
        CFArrayRef (*copySpaces)(int)=dlsym(RTLD_DEFAULT,"SLSCopyManagedDisplaySpaces");
        if(!copySpaces)return 1;
        CFArrayRef spaces=copySpaces(sky.connection());if(!spaces)return 1;
        reply(@{@"displays":(__bridge NSArray *)spaces});CFRelease(spaces);return 0;
    }
    if(argc==3&&strcmp(argv[1],"--space")==0) {
        NSString *identifier=strcmp(argv[2],"next")==0?@"81":strcmp(argv[2],"previous")==0?@"79":nil;
        if(!identifier)return 1;
        NSDictionary *domain=[NSUserDefaults.standardUserDefaults persistentDomainForName:@"com.apple.symbolichotkeys"];
        NSDictionary *hotkey=domain[@"AppleSymbolicHotKeys"][identifier];NSArray *parameters=hotkey[@"value"][@"parameters"];
        if(![hotkey[@"enabled"] boolValue]||parameters.count!=3)return 1;
        CGKeyCode key=[parameters[1] unsignedShortValue];CGEventFlags flags=[parameters[2] unsignedLongLongValue];
        CGEventRef modifierDown=CGEventCreateKeyboardEvent(NULL,59,true),modifierUp=CGEventCreateKeyboardEvent(NULL,59,false);
        CGEventSetFlags(modifierDown,kCGEventFlagMaskControl);CGEventSetFlags(modifierUp,0);
        CGEventRef down=CGEventCreateKeyboardEvent(NULL,key,true),up=CGEventCreateKeyboardEvent(NULL,key,false);
        CGEventSetFlags(down,flags);CGEventSetFlags(up,flags);
        CGEventPost(kCGHIDEventTap,modifierDown);usleep(25000);CGEventPost(kCGHIDEventTap,down);usleep(25000);
        CGEventPost(kCGHIDEventTap,up);CGEventPost(kCGHIDEventTap,modifierUp);
        CFRelease(modifierDown);CFRelease(modifierUp);CFRelease(down);CFRelease(up);return 0;
    }
    if(argc==3&&strcmp(argv[1],"--key")==0) {
        char *end=NULL;long pid=strtol(argv[2],&end,10);
        if(pid<=0||*end||NSWorkspace.sharedWorkspace.frontmostApplication.processIdentifier!=pid)return 1;
        CGEventRef down=CGEventCreateKeyboardEvent(NULL,7,true),up=CGEventCreateKeyboardEvent(NULL,7,false);
        UniChar character='x';CGEventKeyboardSetUnicodeString(down,1,&character);
        CGEventSetFlags(down,0);CGEventSetFlags(up,0);
        CGEventPost(kCGHIDEventTap,down);CGEventPost(kCGHIDEventTap,up);CFRelease(down);CFRelease(up);return 0;
    }
    if(argc==2) {
        char *end=NULL;unsigned long id=strtoul(argv[1],&end,10);SkyLight sky;
        if(!id||id>UINT32_MAX||*end||!loadSkyLight(&sky))return 1;
        NSDictionary *state=windowState(sky,(uint32_t)id);reply(state);
        return [state[@"errors"][0] intValue]||[state[@"errors"][1] intValue]?1:0;
    }
    NSDictionary *configuration=nil;
    if(argc==3&&strcmp(argv[1],"--fixture")==0) {
        NSData *data=[@(argv[2]) dataUsingEncoding:NSUTF8StringEncoding];
        configuration=[NSJSONSerialization JSONObjectWithData:data options:0 error:nil];
        if(![configuration isKindOfClass:NSDictionary.class])return 1;
    } else if(argc!=1)return 1;
    NSApplication *app=NSApplication.sharedApplication;
    [app setActivationPolicy:[configuration[@"regular"] boolValue]?NSApplicationActivationPolicyRegular:NSApplicationActivationPolicyAccessory];
    Fixture *fixture=[Fixture new];fixture.configuration=configuration;app.delegate=fixture;[app run];return 0;
}}
