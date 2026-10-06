// Actual payload frame handler against an owned surface, without Dock injection.
#import "payload.m"
static void check(BOOL condition,const char *message) {
    if(!condition){fprintf(stderr,"FAIL: %s\n",message);exit(1);}
    printf("PASS: %s\n",message);
}
static NSDictionary *rectangle(double x,double y,double width,double height) {
    return @{@"x":@(x),@"y":@(y),@"width":@(width),@"height":@(height)};
}
int main(void) { @autoreleasepool {
    [NSApplication sharedApplication];[NSApp setActivationPolicy:NSApplicationActivationPolicyAccessory];
    [NSApp finishLaunching];
    check(loadSkyLight(&sky),"load own-window compositor API");
    void *library=dlopen("/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight",RTLD_NOW);
    getOwner=dlsym(library,"SLSGetWindowOwner");connectionPID=dlsym(library,"SLSConnectionGetPID");
    saved=[NSMutableDictionary dictionary];stickySaved=[NSMutableDictionary dictionary];
    NSWindow *window=[[NSWindow alloc] initWithContentRect:NSMakeRect(300,300,400,400)
        styleMask:NSWindowStyleMaskBorderless backing:NSBackingStoreBuffered defer:NO];
    window.releasedWhenClosed=NO;[window orderFrontRegardless];[window displayIfNeeded];
    [[NSRunLoop mainRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.1]];
    uint32_t wid=(uint32_t)window.windowNumber;
    check(wid>0&&ownerPID(wid)==getpid(),"actual surface belongs to this test process");
    CGAffineTransform original;check(!sky.getTransform(sky.connection(),wid,&original),"snapshot original transform");
    NSMutableDictionary *update=[@{@"wid":@(wid),@"pid":@(getpid()),@"frame":rectangle(200,200,400,400),
        @"clip":rectangle(200,200,400,400)} mutableCopy];
    NSDictionary *request=@{@"session":@"owned-interactive-test",@"updates":@[update]};
    check([frame(request)[@"ok"] boolValue],"normal placement accepted");
    update[@"viewport"]=rectangle(100,100,900,700);
    CGError (*regionBounds)(CFTypeRef,CGRect *)=dlsym(RTLD_DEFAULT,"CGSGetRegionBounds");
    for(int i=0;i<10;i++) {
        double x=850+i*20;
        CGAffineTransform owner=CGAffineTransformMakeTranslation(-x,-220);
        check(!sky.setTransform(sky.connection(),wid,owner),"owner moves surface during hold");
        check([frame(request)[@"ok"] boolValue],"interactive lease accepted");
        CGAffineTransform actual;sky.getTransform(sky.connection(),wid,&actual);
        check(CGAffineTransformEqualToTransform(owner,actual),"interactive frame preserves owner's transform");
        CFTypeRef clip=NULL;CGRect bounds=CGRectZero;
        check(!sky.copyClip(sky.connection(),wid,&clip)&&clip&&regionBounds&&!regionBounds(clip,&bounds),"read actual interactive clip");
        sky.releaseRegion(clip);
        check(fabs(bounds.size.width-fmax(0,1000-x))<1&&bounds.origin.x==0,"interactive clip stays inside retained viewport");
    }
    [update removeObjectForKey:@"viewport"];
    check([frame(request)[@"ok"] boolValue],"normal placement resumes after release");
    CGAffineTransform released;sky.getTransform(sky.connection(),wid,&released);
    check(released.tx==-200&&released.ty==-200,"release returns surface to layout");
    check(!restoreAll(),"original compositor state restored");
    sky.getTransform(sky.connection(),wid,&released);
    check(CGAffineTransformEqualToTransform(original,released),"original transform restored exactly");
    [window close];return 0;
} }
