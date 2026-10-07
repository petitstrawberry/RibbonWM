// Actual payload frame handler against an owned surface, without Dock injection.
#import "payload.m"
static void check(BOOL condition,const char *message) {
    if(!condition){fprintf(stderr,"FAIL: %s\n",message);exit(1);}
    printf("PASS: %s\n",message);
}
static NSDictionary *rectangle(double x,double y,double width,double height) {
    return @{@"x":@(x),@"y":@(y),@"width":@(width),@"height":@(height)};
}
static CGError (*actualGetBounds)(int,uint32_t,CGRect *);
static uint32_t nullBoundsID;
static CGError transientNullBounds(int cid,uint32_t wid,CGRect *bounds) {
    if(wid==nullBoundsID){*bounds=CGRectNull;return 0;}
    return actualGetBounds(cid,wid,bounds);
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
    NSWindow *child=[[NSWindow alloc] initWithContentRect:NSMakeRect(320,320,66,20)
        styleMask:NSWindowStyleMaskBorderless backing:NSBackingStoreBuffered defer:NO];
    child.releasedWhenClosed=NO;[window addChildWindow:child ordered:NSWindowAbove];[child orderFrontRegardless];
    [[NSRunLoop mainRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.1]];
    uint32_t childID=(uint32_t)child.windowNumber;CGRect parentBounds,childBounds;
    sky.getBounds(sky.connection(),wid,&parentBounds);sky.getBounds(sky.connection(),childID,&childBounds);
    CGFloat dx=childBounds.origin.x-parentBounds.origin.x,dy=childBounds.origin.y-parentBounds.origin.y;
    CGAffineTransform original;check(!sky.getTransform(sky.connection(),wid,&original),"snapshot original transform");
    NSMutableDictionary *update=[@{@"wid":@(wid),@"pid":@(getpid()),@"frame":rectangle(200,200,400,400),
        @"clip":rectangle(200,200,400,400),@"clip_viewport":rectangle(100,100,900,700)} mutableCopy];
    NSDictionary *request=@{@"session":@"owned-interactive-test",@"updates":@[update]};
    check([frame(request)[@"ok"] boolValue],"normal placement accepted");
    check(saved.count==2&&saved[@(childID)].root==wid,"only an explicit attached surface joins its root lease");
    CGAffineTransform childTransform;sky.getTransform(sky.connection(),childID,&childTransform);
    check(fabs(childTransform.tx+200+dx)<1&&fabs(childTransform.ty+200+dy)<1,
        "attached surface keeps its native offset at the transformed parent");
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
        sky.getTransform(sky.connection(),childID,&childTransform);
        check(fabs(childTransform.tx+x+dx)<1&&fabs(childTransform.ty+220+dy)<1,"child follows the owner during a clipped native drag");
    }
    update[@"drag_frame"]=rectangle(240,260,400,400);
    check([frame(request)[@"ok"] boolValue],"pointer drag frame corrects an owner translation offset");
    CGAffineTransform pointed;sky.getTransform(sky.connection(),wid,&pointed);
    check(pointed.tx==-240&&pointed.ty==-260,"pointer drag preserves the grab point on both axes");
    [update removeObjectForKey:@"drag_frame"];
    [update removeObjectForKey:@"viewport"];
    check([frame(request)[@"ok"] boolValue],"normal placement resumes after release");
    CGAffineTransform released;sky.getTransform(sky.connection(),wid,&released);
    check(released.tx==-200&&released.ty==-200,"release returns surface to layout");
    check(!restoreAll(),"original compositor state restored");
    sky.getTransform(sky.connection(),wid,&released);
    check(CGAffineTransformEqualToTransform(original,released),"original transform restored exactly");
    update[@"clip"]=NSNull.null;
    check([frame(request)[@"ok"] boolValue],"fully hidden surface leased");
    check([overview()[@"ok"] boolValue],"overview removes hidden clip without dropping the lease");
    CFTypeRef full=NULL;CGRect fullBounds=CGRectZero;
    check(!sky.copyClip(sky.connection(),wid,&full)&&!regionBounds(full,&fullBounds),"read exposed full clip");
    sky.releaseRegion(full);
    check(fullBounds.size.width==400&&fullBounds.size.height==400&&saved.count==2,"overview exposes complete real surface and retains snapshot");
    sky.getTransform(sky.connection(),wid,&released);
    check(CGAffineTransformEqualToTransform(original,released),"overview returns native transform");
    update[@"clip"]=rectangle(200,200,400,400);
    check([frame(request)[@"ok"] boolValue],"layout resumes after overview");
    NSMutableDictionary *bad=[update mutableCopy];bad[@"pid"]=@(getpid()+1);
    check(![finish(@{@"updates":@[bad]})[@"ok"] boolValue]&&saved.count==2,"release rejects a changed owner before writing");
    update[@"frame"]=rectangle(240,260,400,400);
    check([finish(request)[@"ok"] boolValue]&&saved.count==0,"stop commits desktop geometry and removes the lease");
    check(!restoreAll(),"post-stop cleanup has no obsolete snapshot to apply");
    sky.getTransform(sky.connection(),wid,&released);
    check(released.tx==-240&&released.ty==-260,"post-stop cleanup preserves the released position");
    sky.getTransform(sky.connection(),childID,&childTransform);
    check(fabs(childTransform.tx+240+dx)<1&&fabs(childTransform.ty+260+dy)<1,"finish releases the attached surface at the parent's final position");
    check([frame(request)[@"ok"] boolValue],"lease family again before a native owner move");
    [window setFrameOrigin:NSMakePoint(350,350)];
    [[NSRunLoop mainRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.1]];
    sky.getBounds(sky.connection(),wid,&parentBounds);
    check([overview()[@"ok"] boolValue],"overview accepts owner geometry changed during the lease");
    sky.getTransform(sky.connection(),wid,&released);
    check(fabs(released.tx+parentBounds.origin.x)<1&&fabs(released.ty+parentBounds.origin.y)<1,
        "overview uses current native position rather than an obsolete absolute snapshot");
    check([finish(request)[@"ok"] boolValue]&&saved.count==0,"release moved family without stale child snapshots");
    check([frame(request)[@"ok"] boolValue],"lease before child withdrawal");
    [window removeChildWindow:child];[child orderOut:nil];
    [[NSRunLoop mainRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.1]];
    CGRect hiddenBounds;CGError hiddenError=sky.getBounds(sky.connection(),childID,&hiddenBounds);
    printf("hidden child pid=%d bounds_error=%d bounds=(%g,%g,%g,%g)\n",ownerPID(childID),hiddenError,hiddenBounds.origin.x,hiddenBounds.origin.y,hiddenBounds.size.width,hiddenBounds.size.height);
    check([frame(request)[@"ok"] boolValue],"child withdrawal does not terminate the root lease");
    check(!restoreAll(),"withdrawn child leaves no stuck lease");
    [window addChildWindow:child ordered:NSWindowAbove];[child orderFrontRegardless];
    [[NSRunLoop mainRunLoop] runUntilDate:[NSDate dateWithTimeIntervalSinceNow:0.1]];
    check([frame(request)[@"ok"] boolValue],"lease before transient null child bounds");
    CGRect lastNative=saved[@(childID)].nativeBounds;
    actualGetBounds=sky.getBounds;nullBoundsID=childID;sky.getBounds=transientNullBounds;
    check(!restoreAll()&&saved.count==0&&!controller,"null child bounds cannot strand the controller lease");
    sky.getBounds=actualGetBounds;
    sky.getTransform(sky.connection(),childID,&childTransform);
    check(fabs(childTransform.tx+lastNative.origin.x)<1&&fabs(childTransform.ty+lastNative.origin.y)<1,
        "null bounds release uses last observed finite native position");
    [window removeChildWindow:child];
    [child close];[window close];return 0;
} }
