// Independent AppKit fixture. It never changes WindowServer transforms or clips.
// Only the external controller and Dock payload may perform those operations.
#import "skylight.h"
#include "bridge.h"

static void reply(NSDictionary *value) {
    NSData *data=[NSJSONSerialization dataWithJSONObject:value options:0 error:nil];
    fwrite(data.bytes,1,data.length,stdout);putchar('\n');fflush(stdout);
}
static NSDictionary *windowState(SkyLight sky,uint32_t wid) {
    CGAffineTransform t={0};CGRect b=CGRectZero;
    CGError et=sky.getTransform(sky.connection(),wid,&t),eb=sky.getBounds(sky.connection(),wid,&b);
    CFTypeRef region=NULL;CGRect clip=CGRectZero;
    CGError (*regionBounds)(CFTypeRef,CGRect *)=dlsym(RTLD_DEFAULT,"CGSGetRegionBounds");
    CGError ec=sky.copyClip(sky.connection(),wid,&region);
    if(!ec&&region&&regionBounds)ec=regionBounds(region,&clip);
    if(region)sky.releaseRegion(region);
    NSMutableArray *order=[NSMutableArray array];
    CFArrayRef list=CGWindowListCopyWindowInfo(kCGWindowListOptionAll|kCGWindowListExcludeDesktopElements,kCGNullWindowID);
    for(NSDictionary *row in (__bridge NSArray *)list)if([row[(id)kCGWindowOwnerPID] intValue]==getpid())
        [order addObject:@{@"wid":row[(id)kCGWindowNumber],@"onscreen":row[(id)kCGWindowIsOnscreen]}];
    if(list)CFRelease(list);
    return @{@"event":@"state",@"errors":@[@(et),@(eb)],@"transform":@[@(t.a),@(t.b),@(t.c),@(t.d),@(t.tx),@(t.ty)],
        @"order":order,
        @"clip_error":@(ec),@"clip_bounds":@[@(clip.origin.x),@(clip.origin.y),@(clip.size.width),@(clip.size.height)],
        @"frame":@{@"x":@(b.origin.x),@"y":@(b.origin.y),@"width":@(b.size.width),@"height":@(b.size.height)}};
}
@interface TestPanel : NSPanel
@end
@implementation TestPanel
- (BOOL)canBecomeKeyWindow {return NO;}
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
@property TestPanel *target;
@property NSMutableArray<TestPanel *> *backdrops;
@property NSDictionary *configuration;
@property SkyLight sky;
@property NSRunningApplication *previous;
@end
@implementation Fixture
- (void)stop {
    [self.target close];for(TestPanel *panel in self.backdrops)[panel close];
    if(NSApp.active)[self.previous activateWithOptions:0];
    [NSApp stop:nil];[NSApp postEvent:[NSEvent otherEventWithType:NSEventTypeApplicationDefined
        location:NSZeroPoint modifierFlags:0 timestamp:0 windowNumber:0 context:nil subtype:0 data1:0 data2:0] atStart:NO];
}
- (void)command:(NSDictionary *)command {
    NSString *op=command[@"op"];
    if([op isEqual:@"quit"]) {[self stop];return;}
    if([op isEqual:@"present"]) {self.target.alphaValue=1;self.target.ignoresMouseEvents=NO;reply(@{@"presented":@YES});return;}
    if([op isEqual:@"state"]) {
        NSMutableDictionary *state=[windowState(self.sky,(uint32_t)self.target.windowNumber) mutableCopy];
        state[@"alpha"]=@(self.target.alphaValue);state[@"visible"]=@(self.target.visible);
        state[@"ignores_mouse"]=@(self.target.ignoresMouseEvents);state[@"occlusion"]=@(self.target.occlusionState);
        reply(state);return;
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
    self.target=[[TestPanel alloc] initWithContentRect:NSMakeRect([origin[@"x"] doubleValue],height-[origin[@"y"] doubleValue]-400,400,400) styleMask:NSWindowStyleMaskBorderless|NSWindowStyleMaskNonactivatingPanel backing:NSBackingStoreBuffered defer:NO];
    self.target.level=NSFloatingWindowLevel;self.target.hidesOnDeactivate=NO;self.target.hasShadow=[self.configuration[@"shadow"] boolValue];
    self.target.collectionBehavior=NSWindowCollectionBehaviorMoveToActiveSpace;
    self.target.alphaValue=0;self.target.ignoresMouseEvents=YES;
    TestView *view=[[TestView alloc] initWithFrame:NSMakeRect(0,0,400,400)];view.target=YES;view.surface=@"target";self.target.contentView=view;
    [self.target orderFrontRegardless];[self.target displayIfNeeded];
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
    // External focus event for QA: only the explicitly named disposable
    // Alacritty window is eligible, never a user's ordinary terminal or Codex.
    if((argc==4&&(strcmp(argv[1],"--focus-fixture")==0||strcmp(argv[1],"--click-fixture")==0))||
       (argc==6&&(strcmp(argv[1],"--resize-fixture")==0||strcmp(argv[1],"--drag-fixture")==0))) {
        BOOL click=strcmp(argv[1],"--click-fixture")==0;
        BOOL resize=strcmp(argv[1],"--resize-fixture")==0;
        BOOL drag=strcmp(argv[1],"--drag-fixture")==0;
        char *end=NULL;unsigned long wid=strtoul(argv[2],&end,10);
        if(!wid||wid>UINT32_MAX||*end)return 1;
        long pid=strtol(argv[3],&end,10);if(pid<=0||pid>INT_MAX||*end)return 1;
        NSRunningApplication *app=[NSRunningApplication runningApplicationWithProcessIdentifier:(pid_t)pid];
        // Nix's executable wrapper can launch Alacritty without a bundle ID.
        if(![app.bundleIdentifier isEqual:@"org.alacritty"]&&![app.localizedName.lowercaseString containsString:@"alacritty"]){reply(@{@"error":@"Not the fixture app",@"bundle":app.bundleIdentifier?:@"",@"app":app.localizedName?:@""});return 1;}
        CFArrayRef list=CGWindowListCopyWindowInfo(kCGWindowListOptionAll|kCGWindowListExcludeDesktopElements,kCGNullWindowID);
        BOOL fixture=NO;
        for(NSDictionary *row in (__bridge NSArray *)list) {
            if([row[(id)kCGWindowNumber] unsignedIntValue]==wid&&[row[(id)kCGWindowOwnerPID] intValue]==pid&&
                [row[(id)kCGWindowName] hasPrefix:@"RibbonWM QA "]&&
                (!(click||drag)||[row[(id)kCGWindowIsOnscreen] boolValue])) {fixture=YES;break;}
        }
        if(list)CFRelease(list);
        if(!fixture){reply(@{@"error":@"Not a named fixture window"});return 1;}
        if(resize) {
            double width=strtod(argv[4],&end);if(*end||!isfinite(width)||width<100||width>10000)return 1;
            double height=strtod(argv[5],&end);if(*end||!isfinite(height)||height<100||height>10000)return 1;
            SkyLight sky;if(!loadSkyLight(&sky))return 1;CGRect frame;
            if(sky.getBounds(sky.connection(),(uint32_t)wid,&frame))return 1;
            int result=ribbon_resize_window((uint32_t)wid,(int)pid,(RibbonRect){frame.origin.x,frame.origin.y,width,height});
            reply(@{@"resize_error":@(result),@"state":windowState(sky,(uint32_t)wid)});return result?1:0;
        }
        if(click||drag) {
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
            double dx=0;
            if(drag) {
                dx=strtod(argv[4],&end);if(*end||!isfinite(dx)||fabs(dx)>200||strcmp(argv[5],"0")!=0)return 1;
                if(ribbon_focused_window((int)pid)!=wid||[clip[0] doubleValue]!=0||fabs([clip[2] doubleValue]-[state[@"frame"][@"width"] doubleValue])>1)return 1;
                point.x=-[t[4] doubleValue]+[clip[2] doubleValue]-1;
            }
            CGEventRef move=CGEventCreateMouseEvent(NULL,kCGEventMouseMoved,point,kCGMouseButtonLeft);
            CGEventRef down=CGEventCreateMouseEvent(NULL,kCGEventLeftMouseDown,point,kCGMouseButtonLeft);
            CGEventRef up=CGEventCreateMouseEvent(NULL,kCGEventLeftMouseUp,point,kCGMouseButtonLeft);
            CGEventPost(kCGHIDEventTap,move);usleep(100000);CGEventPost(kCGHIDEventTap,down);usleep(50000);
            if(drag) {
                for(unsigned i=1;i<=10;i++) {
                    CGPoint next=CGPointMake(point.x+dx*i/10,point.y);
                    CGEventRef event=CGEventCreateMouseEvent(NULL,kCGEventLeftMouseDragged,next,kCGMouseButtonLeft);
                    CGEventPost(kCGHIDEventTap,event);CFRelease(event);usleep(50000);
                }
                CGEventSetLocation(up,CGPointMake(point.x+dx,point.y));
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
    NSApplication *app=NSApplication.sharedApplication;[app setActivationPolicy:NSApplicationActivationPolicyAccessory];
    Fixture *fixture=[Fixture new];fixture.configuration=configuration;app.delegate=fixture;[app run];return 0;
}}
