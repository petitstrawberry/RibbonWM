// AppKit owns the test surfaces and event loop. Rust owns all layout/animation policy.
#import "skylight.h"
#import "bridge.h"

static const RibbonCallbacks *callbacks;
@interface RibbonPanel : NSPanel
@end
@implementation RibbonPanel
- (BOOL)canBecomeKeyWindow {return YES;}
- (BOOL)canBecomeMainWindow {return YES;}
@end
@interface RibbonColumnView : NSView
@property int column;
@end
@implementation RibbonColumnView
- (BOOL)acceptsFirstMouse:(NSEvent *)event {(void)event;return YES;}
- (void)mouseDown:(NSEvent *)event {
    NSPoint p=[self convertPoint:event.locationInWindow fromView:nil];
    printf("click column=%d local=(%.1f,%.1f)\n",self.column,p.x,p.y); fflush(stdout);
    [[NSNotificationCenter defaultCenter] postNotificationName:@"RibbonClick" object:self userInfo:@{@"x":@(p.x),@"y":@(p.y)}];
}
- (void)drawRect:(NSRect)dirty {
    (void)dirty;
    [[NSColor colorWithCalibratedHue:0.12+self.column*0.22 saturation:0.65 brightness:0.8 alpha:1] setFill]; NSRectFill(self.bounds);
    [[NSString stringWithFormat:@"Column %d",self.column+1] drawAtPoint:NSMakePoint(25,300) withAttributes:@{
        NSFontAttributeName:[NSFont monospacedSystemFontOfSize:24 weight:NSFontWeightBold],NSForegroundColorAttributeName:NSColor.whiteColor}];
    [@"A real native window, driven by Rust" drawAtPoint:NSMakePoint(25,250) withAttributes:@{
        NSFontAttributeName:[NSFont systemFontOfSize:16],NSForegroundColorAttributeName:NSColor.whiteColor}];
    for (int i=0;i<8;i++) [[NSString stringWithFormat:@"x=%d",i*50] drawAtPoint:NSMakePoint(i*50,180) withAttributes:@{
        NSFontAttributeName:[NSFont systemFontOfSize:12],NSForegroundColorAttributeName:NSColor.whiteColor}];
}
@end
@interface RibbonBackdropView : NSView
@end
@implementation RibbonBackdropView
- (BOOL)acceptsFirstMouse:(NSEvent *)event {(void)event;return YES;}
- (void)mouseDown:(NSEvent *)event {
    NSPoint p=[self convertPoint:event.locationInWindow fromView:nil];
    printf("backdrop click local=(%.1f,%.1f)\n",p.x,p.y);fflush(stdout);
    [[NSNotificationCenter defaultCenter] postNotificationName:@"RibbonBackdropClick" object:self userInfo:@{@"x":@(p.x),@"y":@(p.y)}];
}
@end

@interface RibbonDemo : NSObject <NSApplicationDelegate>
@property SkyLight sky;
@property NSMutableArray<NSPanel *> *panels;
@property NSPanel *backdrop;
@property NSTimer *timer;
@property int result;
@property BOOL test,finished,presented,clicked,clipClicked,hiddenClicked,sentClick,sentClipClick,sentHiddenClick;
@property double began,lastTick,seconds,left,width;
@property id keyMonitor,scrollMonitor,mouseMonitor,clickObserver,backdropObserver;
@property NSRunningApplication *previousApplication;
@end
@implementation RibbonDemo {
    CGAffineTransform original[3];
    CFTypeRef clips[3];
    RibbonDemoInfo info;
    RibbonDemoFrame latest[3];
}
- (void)stop {
    self.finished=YES; [NSApp stop:nil];
    [NSApp postEvent:[NSEvent otherEventWithType:NSEventTypeApplicationDefined location:NSZeroPoint modifierFlags:0
        timestamp:0 windowNumber:0 context:nil subtype:0 data1:0 data2:0] atStart:NO];
}
- (void)applicationDidFinishLaunching:(NSNotification *)n {
    (void)n;
    if (!loadSkyLight(&_sky)) {self.result=1; [self stop]; return;}
    self.previousApplication=NSWorkspace.sharedWorkspace.frontmostApplication;
    NSRect screen=NSScreen.mainScreen.visibleFrame;
    CGFloat width=MIN(self.width,screen.size.width-self.left-40);
    if(width<548 || screen.size.height<560) {fprintf(stderr,"Demo does not fit the selected screen area\n");self.result=1;[self stop];return;}
    NSRect outer=NSMakeRect(screen.origin.x+self.left,screen.origin.y+80,width,480);
    self.backdrop=[[RibbonPanel alloc] initWithContentRect:outer styleMask:NSWindowStyleMaskBorderless backing:NSBackingStoreBuffered defer:NO];
    self.backdrop.level=NSFloatingWindowLevel; self.backdrop.hidesOnDeactivate=NO; self.backdrop.hasShadow=NO;
    self.backdrop.collectionBehavior=NSWindowCollectionBehaviorMoveToActiveSpace;
    self.backdrop.backgroundColor=[NSColor colorWithWhite:0.08 alpha:1];
    self.backdrop.contentView=[[RibbonBackdropView alloc] initWithFrame:NSMakeRect(0,0,width,480)];
    NSTextField *label=[NSTextField labelWithString:@"RibbonWM  |  ← → columns  ·  Esc exits"];
    label.textColor=NSColor.whiteColor; label.frame=NSMakeRect(15,440,width-30,25); [self.backdrop.contentView addSubview:label];
    [self.backdrop orderFrontRegardless]; [self.backdrop displayIfNeeded];
    [NSRunLoop.currentRunLoop runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.15]];
    CGRect backdropBounds;
    if (self.sky.getBounds(self.sky.connection(),(uint32_t)self.backdrop.windowNumber,&backdropBounds)) {self.result=1;[self stop];return;}
    info.viewport=(RibbonRect){backdropBounds.origin.x+24,backdropBounds.origin.y+50,width-48,400};
    printf("viewport=(%.0f,%.0f,%.0f,%.0f)\n",info.viewport.x,info.viewport.y,info.viewport.width,info.viewport.height);
    fflush(stdout);
    self.panels=[NSMutableArray array];
    for (int i=0;i<3;i++) {
        NSPanel *w=[[RibbonPanel alloc] initWithContentRect:NSMakeRect(outer.origin.x+24+i*420,outer.origin.y+30,400,400)
            styleMask:NSWindowStyleMaskBorderless backing:NSBackingStoreBuffered defer:NO];
        w.level=NSFloatingWindowLevel; w.hidesOnDeactivate=NO; w.hasShadow=NO;
        // Create the WindowServer surfaces invisibly until Rust has applied the first clip.
        w.alphaValue=0;
        w.ignoresMouseEvents=YES;
        w.collectionBehavior=NSWindowCollectionBehaviorMoveToActiveSpace;
        RibbonColumnView *v=[[RibbonColumnView alloc] initWithFrame:NSMakeRect(0,0,400,400)];v.column=i;w.contentView=v;
        [w orderFrontRegardless];[w displayIfNeeded];[self.panels addObject:w];
        [NSRunLoop.currentRunLoop runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.15]];
        uint32_t wid=(uint32_t)w.windowNumber; CGRect b;
        CGError eb=self.sky.getBounds(self.sky.connection(),wid,&b);
        CGError et=self.sky.getTransform(self.sky.connection(),wid,&original[i]);
        CGError ec=self.sky.copyClip(self.sky.connection(),wid,&clips[i]);
        if (eb || et || ec || !clips[i]) {self.result=1;[self stop];return;}
        info.windows[i]=(RibbonDemoWindow){wid,{b.origin.x,b.origin.y,b.size.width,b.size.height}};
        if(self.test)printf("window %d id=%u bounds=(%.0f,%.0f,%.0f,%.0f) transform=(%.0f,%.0f)\n",i,wid,b.origin.x,b.origin.y,b.size.width,b.size.height,original[i].tx,original[i].ty);
    }
    if (callbacks->initialize(callbacks->context,&info)) {self.result=1;[self stop];return;}
    __weak RibbonDemo *weak=self;
    if(self.test)self.mouseMonitor=[NSEvent addLocalMonitorForEventsMatchingMask:NSEventMaskLeftMouseDown handler:^NSEvent *(NSEvent *e) {
        printf("mouse event window=%ld local=(%.1f,%.1f)\n",(long)e.windowNumber,e.locationInWindow.x,e.locationInWindow.y);fflush(stdout);return e;
    }];
    self.keyMonitor=[NSEvent addLocalMonitorForEventsMatchingMask:NSEventMaskKeyDown handler:^NSEvent *(NSEvent *e) {
        if (e.keyCode==53 || [e.charactersIgnoringModifiers isEqual:@"q"]) {[weak stop];return nil;}
        if (e.keyCode==123 || e.keyCode==124) {callbacks->event(callbacks->context,1,e.keyCode==124?1:-1);return nil;}
        return e;
    }];
    self.scrollMonitor=[NSEvent addLocalMonitorForEventsMatchingMask:NSEventMaskScrollWheel handler:^NSEvent *(NSEvent *e) {
        if (fabs(e.scrollingDeltaX)>0.01) {callbacks->event(callbacks->context,4,-e.scrollingDeltaX);return nil;} return e;
    }];
    self.clickObserver=[[NSNotificationCenter defaultCenter] addObserverForName:@"RibbonClick" object:nil queue:nil usingBlock:^(NSNotification *n) {
        RibbonColumnView *v=n.object;
        CGFloat x=[n.userInfo[@"x"] doubleValue],y=[n.userInfo[@"y"] doubleValue];
        if (weak.test) weak.clicked=v.column==2 && fabs(x-100)<2 && fabs(y-200)<2;
        else callbacks->event(callbacks->context,5,v.column);
    }];
    self.backdropObserver=[[NSNotificationCenter defaultCenter] addObserverForName:@"RibbonBackdropClick" object:nil queue:nil usingBlock:^(NSNotification *n) {
        double x=[n.userInfo[@"x"] doubleValue],y=[n.userInfo[@"y"] doubleValue];
        if (weak.sentHiddenClick && fabs(x-424)<2 && fabs(y-230)<2) weak.hiddenClicked=YES;
        else if (weak.sentClipClick && !weak.sentHiddenClick && fabs(x-12)<2 && fabs(y-230)<2) weak.clipClicked=YES;
    }];
    self.began=self.lastTick=NSProcessInfo.processInfo.systemUptime;
    self.timer=[NSTimer scheduledTimerWithTimeInterval:1.0/60 repeats:YES block:^(NSTimer *t) {(void)t;[weak tick];}];
    // Float the surfaces without changing other applications' activation or window ordering.
    fflush(stdout);
}
- (void)clickAt:(CGPoint)point {
    CGEventRef move=CGEventCreateMouseEvent(NULL,kCGEventMouseMoved,point,kCGMouseButtonLeft);
    CGEventPost(kCGHIDEventTap,move);CFRelease(move);
    // Let WindowServer update mouse focus before the button events, as with a real click.
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW,30*NSEC_PER_MSEC),dispatch_get_main_queue(),^{
        CGEventRef down=CGEventCreateMouseEvent(NULL,kCGEventLeftMouseDown,point,kCGMouseButtonLeft);
        CGEventPost(kCGHIDEventTap,down);CFRelease(down);
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW,30*NSEC_PER_MSEC),dispatch_get_main_queue(),^{
            CGEventRef up=CGEventCreateMouseEvent(NULL,kCGEventLeftMouseUp,point,kCGMouseButtonLeft);
            CGEventPost(kCGHIDEventTap,up);CFRelease(up);
        });
    });
}
- (void)tick {
    double now=NSProcessInfo.processInfo.systemUptime,elapsed=now-self.began;
    if (self.test && elapsed>3.0) {
        self.result=self.clicked&&self.clipClicked&&self.hiddenClicked?0:1;
        printf("transformed input: %s\nviewport clipped input: %s\nempty clip input: %s\n",
            self.clicked?"PASS":"FAIL",self.clipClicked?"PASS":"FAIL",self.hiddenClicked?"PASS":"FAIL");
        [self stop];return;
    }
    if (self.seconds>0 && elapsed>self.seconds) {[self stop];return;}
    size_t count=callbacks->update(callbacks->context,now-self.lastTick,latest,3);self.lastTick=now;
    if (count!=3) {self.result=1;[self stop];return;}
    for (size_t i=0;i<count;i++) {
        int index=-1;for (int j=0;j<3;j++) if (info.windows[j].wid==latest[i].wid) index=j;
        if (index<0) {self.result=1;[self stop];return;}
        RibbonDemoFrame f=latest[i]; RibbonRect b=info.windows[index].bounds;
        CGRect rect=f.visible?CGRectMake(f.clip.x-b.x-f.dx,f.clip.y-b.y-f.dy,f.clip.width,f.clip.height):CGRectZero;
        CFTypeRef region=NULL;
        CGError er=self.sky.newRegion(&rect,&region);
        CGError ec=er?er:self.sky.setClip(self.sky.connection(),f.wid,region);
        CGError et=ec?ec:self.sky.setTransform(self.sky.connection(),f.wid,translatedTransform(original[index],f.dx,f.dy));
        if (region) self.sky.releaseRegion(region);
        if (ec || et) {fprintf(stderr,"clip=%d transform=%d\n",ec,et);self.result=1;[self stop];return;}
    }
    if(!self.presented) {
        for(NSPanel *panel in self.panels) {panel.alphaValue=1;panel.ignoresMouseEvents=NO;}
        self.presented=YES;
    }
    if (self.test && elapsed>0.8 && !self.sentClick) {
        self.sentClick=YES;
        for (int i=0;i<3;i++) if (latest[i].wid==info.windows[2].wid) {
            printf("transformed click global=(%.1f,%.1f) dx=%.1f dy=%.1f\n",
                info.windows[2].bounds.x+latest[i].dx+100,info.windows[2].bounds.y+latest[i].dy+200,latest[i].dx,latest[i].dy);fflush(stdout);
            [self clickAt:CGPointMake(info.windows[2].bounds.x+latest[i].dx+100,info.windows[2].bounds.y+latest[i].dy+200)];
        }
    }
    if (self.test && elapsed>1.4 && !self.sentClipClick) {
        self.sentClipClick=YES;[self clickAt:CGPointMake(info.viewport.x-12,info.viewport.y+200)];
    }
    if (self.test && elapsed>2.0 && !self.sentHiddenClick) {
        self.sentHiddenClick=YES;callbacks->event(callbacks->context,2,1);
        // Give Rust and WindowServer one frame to apply the new workspace.
        dispatch_after(dispatch_time(DISPATCH_TIME_NOW,100*NSEC_PER_MSEC),dispatch_get_main_queue(),^{
            [self clickAt:CGPointMake(info.viewport.x+400,info.viewport.y+200)];
        });
    }
}
- (void)finish {
    BOOL restoreFocus=NSApp.active;
    [self.timer invalidate];
    // Restoration must not briefly reveal the original, unclipped window positions.
    for(NSPanel *panel in self.panels) {panel.ignoresMouseEvents=YES;panel.alphaValue=0;}
    if (self.keyMonitor) [NSEvent removeMonitor:self.keyMonitor];
    if (self.scrollMonitor) [NSEvent removeMonitor:self.scrollMonitor];
    if (self.mouseMonitor) [NSEvent removeMonitor:self.mouseMonitor];
    if (self.clickObserver) [[NSNotificationCenter defaultCenter] removeObserver:self.clickObserver];
    if (self.backdropObserver) [[NSNotificationCenter defaultCenter] removeObserver:self.backdropObserver];
    for (NSUInteger i=0;i<self.panels.count;i++) {
        if (clips[i]) {self.sky.setTransform(self.sky.connection(),(uint32_t)self.panels[i].windowNumber,original[i]);
            self.sky.setClip(self.sky.connection(),(uint32_t)self.panels[i].windowNumber,clips[i]);self.sky.releaseRegion(clips[i]);}
        [self.panels[i] close];
    }
    [self.backdrop close]; puts("Demo surfaces restored and closed.");
    if(restoreFocus)[self.previousApplication activateWithOptions:NSApplicationActivateIgnoringOtherApps];
}
@end
int ribbon_demo_run(const RibbonCallbacks *cb,int test,double seconds,double left,double width) { @autoreleasepool {
    callbacks=cb;NSApplication *app=NSApplication.sharedApplication;[app setActivationPolicy:NSApplicationActivationPolicyAccessory];
    RibbonDemo *delegate=[RibbonDemo new];delegate.test=test!=0;delegate.seconds=seconds;delegate.left=left;delegate.width=width;app.delegate=delegate;
    [app run];
    if(delegate.test && (!delegate.clicked || !delegate.clipClicked || !delegate.hiddenClicked))delegate.result=1;
    [delegate finish];app.delegate=nil;callbacks=NULL;return delegate.result;
} }
