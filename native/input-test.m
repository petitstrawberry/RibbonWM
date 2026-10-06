// Owned-window transport test. Synthetic wheel phases are not a finger test.
#import <Cocoa/Cocoa.h>
#include "bridge.h"
static int seen,downstream;
static RibbonScrollEvent packets[8];
static CGPoint point;
static BOOL own(CGEventRef event) {
    CGPoint p=CGEventGetLocation(event);
    return fabs(p.x-point.x)<1&&fabs(p.y-point.y)<1;
}
static int receive(void *context,const RibbonScrollEvent *sample) {
    (void)context;
    if(sample->kind||fabs(sample->x-point.x)>=1||fabs(sample->y-point.y)>=1)return 0;
    if(seen<8)packets[seen++]=*sample;
    return sample->dx<0; // One event consumed, the others pass to the owned app.
}
static CGEventRef after(CGEventTapProxy proxy,CGEventType type,CGEventRef event,void *context) {
    (void)proxy;(void)context;
    if(type==kCGEventScrollWheel&&own(event))downstream++;
    return event;
}
static void pump(void) {
    CFAbsoluteTime until=CFAbsoluteTimeGetCurrent()+0.08;
    while(CFAbsoluteTimeGetCurrent()<until)CFRunLoopRunInMode(kCFRunLoopDefaultMode,0.005,true);
}
static void post(int delta,int phase,int momentum) {
    CGEventRef event=CGEventCreateScrollWheelEvent(NULL,kCGScrollEventUnitPixel,2,0,delta);
    CGEventSetLocation(event,point);
    CGEventSetIntegerValueField(event,kCGScrollWheelEventScrollPhase,phase);
    CGEventSetIntegerValueField(event,kCGScrollWheelEventMomentumPhase,momentum);
    CGEventSetIntegerValueField(event,kCGScrollWheelEventIsContinuous,1);
    CGEventPost(kCGHIDEventTap,event);CFRelease(event);pump();
}
int main(void) { @autoreleasepool {
    if(!ribbon_input_trusted()){fprintf(stderr,"Input Monitoring unavailable; not tested\n");return 2;}
    [NSApplication sharedApplication];[NSApp setActivationPolicy:NSApplicationActivationPolicyAccessory];
    [NSApp finishLaunching];
    NSWindow *window=[[NSWindow alloc]initWithContentRect:NSMakeRect(400,400,400,260)
        styleMask:NSWindowStyleMaskTitled backing:NSBackingStoreBuffered defer:NO];
    window.title=@"RibbonWM QA input transport";[window makeKeyAndOrderFront:nil];
    [NSApp activateIgnoringOtherApps:YES];pump();
    NSScreen *primary=NSScreen.screens.firstObject;
    point=CGPointMake(NSMidX(window.frame),NSMaxY(primary.frame)-NSMidY(window.frame));
    if(!ribbon_start_scroll(receive,NULL)){[window orderOut:nil];return 2;}
    CFMachPortRef observer=CGEventTapCreate(kCGSessionEventTap,kCGTailAppendEventTap,kCGEventTapOptionListenOnly,
        CGEventMaskBit(kCGEventScrollWheel),after,NULL);
    if(!observer){ribbon_stop_scroll();[window orderOut:nil];return 2;}
    CFRunLoopSourceRef source=CFMachPortCreateRunLoopSource(NULL,observer,0);
    CFRunLoopAddSource(CFRunLoopGetMain(),source,kCFRunLoopDefaultMode);
    CGEventRef current=CGEventCreate(NULL);CGPoint saved=CGEventGetLocation(current);CFRelease(current);
    CGEventRef move=CGEventCreateMouseEvent(NULL,kCGEventMouseMoved,point,kCGMouseButtonLeft);
    CGEventPost(kCGHIDEventTap,move);CFRelease(move);pump();
    post(-7,kCGScrollPhaseBegan,0);
    post(4,kCGScrollPhaseEnded,0);
    post(2,0,kCGMomentumScrollPhaseBegin);
    post(1,0,kCGMomentumScrollPhaseEnd);
    BOOL ok=seen==4&&downstream==3&&packets[0].phase==NSEventPhaseBegan
        &&packets[1].phase==NSEventPhaseEnded&&packets[2].momentum==NSEventPhaseBegan
        &&packets[3].momentum==NSEventPhaseEnded&&packets[0].precise&&packets[0].dx==-7
        &&fabs(ribbon_input_time()-packets[3].time)<1.0;
    printf("wheel transport: seen=%d downstream=%d phase=%u,%u momentum=%u,%u dx=%.2f precise=%u => %s\n",
        seen,downstream,packets[0].phase,packets[1].phase,packets[2].momentum,packets[3].momentum,
        packets[0].dx,packets[0].precise,ok?"PASS":"FAIL");
    ribbon_stop_scroll();CFMachPortInvalidate(observer);
    CFRunLoopRemoveSource(CFRunLoopGetMain(),source,kCFRunLoopDefaultMode);CFRelease(source);CFRelease(observer);
    [window orderOut:nil];
    move=CGEventCreateMouseEvent(NULL,kCGEventMouseMoved,saved,kCGMouseButtonLeft);
    CGEventPost(kCGHIDEventTap,move);CFRelease(move);
    return ok?0:1;
} }
