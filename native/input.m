// Value-only input transport. Rust decides which events to consume and where.
#import <Cocoa/Cocoa.h>
#include "bridge.h"
#include <mach/mach_time.h>
_Static_assert(sizeof(RibbonTouch)==32,"Rust touch ABI");
_Static_assert(sizeof(RibbonScrollEvent)==328,"Rust input ABI");
static CFMachPortRef scrollTap;
static CFRunLoopSourceRef scrollSource;
static RibbonScrollCallback scrollCallback;
static void *scrollContext;
static NSMutableArray *touchIdentities;
static CGEventRef scrollEvent(CGEventTapProxy proxy,CGEventType type,CGEventRef event,void *context) {
    (void)proxy;(void)context;
    if(type==kCGEventTapDisabledByTimeout||type==kCGEventTapDisabledByUserInput) {
        if(scrollTap)CGEventTapEnable(scrollTap,true);
        return event;
    }
    if((type!=kCGEventScrollWheel&&type!=(CGEventType)NSEventTypeGesture)||!scrollCallback)return event;
    @autoreleasepool {
        NSEvent *native=[NSEvent eventWithCGEvent:event];if(!native)return event;
        CGPoint point=CGEventGetLocation(event);
        RibbonScrollEvent sample={0};
        // Real momentum packets can carry a driver/synthetic timestamp from a
        // different epoch. Expire captures using receipt time, not that field.
        sample.x=point.x;sample.y=point.y;sample.time=ribbon_input_time();
        sample.flags=CGEventGetFlags(event);sample.phase=(uint32_t)native.phase;
        if(type==kCGEventScrollWheel) {
            sample.dx=native.scrollingDeltaX;sample.dy=native.scrollingDeltaY;
            sample.momentum=(uint32_t)native.momentumPhase;sample.precise=native.hasPreciseScrollingDeltas;
        } else {
            sample.kind=1;
            if(sample.phase&NSEventPhaseBegan)touchIdentities=nil;
            if(touchIdentities.count>64)touchIdentities=nil;
            if(!touchIdentities)touchIdentities=[NSMutableArray array];
            for(NSTouch *touch in native.allTouches) {
                if(sample.count==8)break;
                NSUInteger index=[touchIdentities indexOfObject:touch.identity];
                if(index==NSNotFound){index=touchIdentities.count;[touchIdentities addObject:touch.identity];}
                NSPoint position=touch.normalizedPosition;
                sample.touches[sample.count++]=(RibbonTouch){index+1,position.x,position.y,(uint32_t)touch.phase};
            }
        }
        BOOL consume=scrollCallback(scrollContext,&sample);
        if(sample.kind&&((sample.phase&(NSEventPhaseEnded|NSEventPhaseCancelled))||sample.count==0))touchIdentities=nil;
        return consume?NULL:event;
    }
}
int ribbon_start_scroll(RibbonScrollCallback callback,void *context) {
    if(scrollTap||!callback)return 0;
    scrollCallback=callback;scrollContext=context;
    scrollTap=CGEventTapCreate(kCGHIDEventTap,kCGHeadInsertEventTap,kCGEventTapOptionDefault,
        CGEventMaskBit(kCGEventScrollWheel)|CGEventMaskBit(NSEventTypeGesture),scrollEvent,NULL);
    if(!scrollTap){scrollCallback=NULL;scrollContext=NULL;return 0;}
    scrollSource=CFMachPortCreateRunLoopSource(NULL,scrollTap,0);
    if(!scrollSource){ribbon_stop_scroll();return 0;}
    CFRunLoopAddSource(CFRunLoopGetMain(),scrollSource,kCFRunLoopDefaultMode);
    CGEventTapEnable(scrollTap,true);return 1;
}
void ribbon_stop_scroll(void) {
    if(scrollTap){CGEventTapEnable(scrollTap,false);CFMachPortInvalidate(scrollTap);}
    if(scrollSource){CFRunLoopRemoveSource(CFRunLoopGetMain(),scrollSource,kCFRunLoopDefaultMode);CFRelease(scrollSource);}
    if(scrollTap)CFRelease(scrollTap);
    scrollTap=NULL;scrollSource=NULL;scrollCallback=NULL;scrollContext=NULL;
    touchIdentities=nil;
}
int ribbon_scroll_alive(void) {
    if(!scrollTap||!CFMachPortIsValid(scrollTap))return 0;
    if(!CGEventTapIsEnabled(scrollTap))CGEventTapEnable(scrollTap,true);
    return CGEventTapIsEnabled(scrollTap);
}
double ribbon_input_time(void) {
    mach_timebase_info_data_t info;mach_timebase_info(&info);
    return (double)mach_absolute_time()*info.numer/info.denom/1e9;
}
int ribbon_input_trusted(void) {return CGPreflightListenEventAccess();}
int ribbon_request_input_permission(void) {return CGRequestListenEventAccess();}

// AppKit renders the diagnostic values and transports button actions. Rust
// controls recording, its deadline and the reported results.
static NSWindow *probeWindow;
static NSTextField *probeLabel;
static NSButton *probeButton;
static uint32_t probeAction;
@interface RibbonProbe : NSObject <NSWindowDelegate>
- (void)start:(id)sender;
@end
@implementation RibbonProbe
- (void)start:(id)sender {(void)sender;probeAction|=1;probeButton.enabled=NO;}
- (void)windowWillClose:(NSNotification *)note {(void)note;probeAction|=2;}
@end
static RibbonProbe *probeDelegate;
void ribbon_probe_show(const char *text) { @autoreleasepool {
    if(!probeWindow) {
        [NSApplication sharedApplication];[NSApp setActivationPolicy:NSApplicationActivationPolicyAccessory];
        [NSApp finishLaunching];probeDelegate=[RibbonProbe new];
        probeWindow=[[NSWindow alloc]initWithContentRect:NSMakeRect(0,0,590,350)
            styleMask:NSWindowStyleMaskTitled|NSWindowStyleMaskClosable
            backing:NSBackingStoreBuffered defer:NO];
        probeWindow.releasedWhenClosed=NO;probeWindow.animationBehavior=NSWindowAnimationBehaviorNone;
        probeWindow.title=@"RibbonWM — ジェスチャー計測";probeWindow.delegate=probeDelegate;
        probeWindow.level=NSFloatingWindowLevel;
        NSRect screen=NSScreen.screens.firstObject.visibleFrame;
        [probeWindow setFrameOrigin:NSMakePoint(NSMidX(screen)-NSWidth(probeWindow.frame)/2,
            NSMidY(screen)-NSHeight(probeWindow.frame)/2)];
        probeWindow.collectionBehavior=NSWindowCollectionBehaviorCanJoinAllSpaces|NSWindowCollectionBehaviorFullScreenAuxiliary;
        probeLabel=[NSTextField wrappingLabelWithString:@""];
        probeLabel.font=[NSFont monospacedSystemFontOfSize:15 weight:NSFontWeightRegular];
        probeLabel.frame=NSMakeRect(24,80,540,240);[probeWindow.contentView addSubview:probeLabel];
        probeButton=[NSButton buttonWithTitle:@"計測開始" target:probeDelegate action:@selector(start:)];
        probeButton.frame=NSMakeRect(220,20,150,40);[probeWindow.contentView addSubview:probeButton];
        [probeWindow makeKeyAndOrderFront:nil];[NSApp activateIgnoringOtherApps:YES];
    }
    probeLabel.stringValue=text?@(text):@"";
} }
uint32_t ribbon_probe_action(void) { @autoreleasepool {
    for(int i=0;i<16;i++) {
        NSEvent *event=[NSApp nextEventMatchingMask:NSEventMaskAny untilDate:[NSDate distantPast]
            inMode:NSDefaultRunLoopMode dequeue:YES];
        if(!event)break;[NSApp sendEvent:event];
    }
    [NSApp updateWindows];uint32_t result=probeAction;probeAction=0;return result;
} }
void ribbon_probe_close(void) {
    [probeWindow orderOut:nil];probeWindow=nil;probeLabel=nil;probeButton=nil;probeDelegate=nil;probeAction=0;
}
