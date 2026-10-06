// Notification transport only. Rust chooses eligible apps and owns layout policy.
#import <Cocoa/Cocoa.h>
#import <ApplicationServices/ApplicationServices.h>
#include "bridge.h"
static NSMutableDictionary<NSNumber *,id> *observers;
static NSMutableArray *workspaceObservers;
static uint32_t pendingEvents;
static CFMutableDictionaryRef watchedElements;
static NSMutableDictionary<NSNumber *,NSNumber *> *closedWindows;
static double lastMembershipCheck;
void ribbon_watch_ax_element(const void *element,int pid) {
    AXObserverRef observer=(__bridge AXObserverRef)observers[@(pid)];
    if(!observer)return;
    uint32_t wid=ribbon_ax_window_id(element);
    if(wid) {
        if(!watchedElements)watchedElements=CFDictionaryCreateMutable(NULL,0,&kCFTypeDictionaryKeyCallBacks,&kCFTypeDictionaryValueCallBacks);
        CFDictionarySetValue(watchedElements,element,(__bridge const void *)@{@"wid":@(wid),@"pid":@(pid)});
    }
    for(NSString *name in @[(__bridge NSString *)kAXWindowMovedNotification,
        (__bridge NSString *)kAXWindowResizedNotification,(__bridge NSString *)kAXUIElementDestroyedNotification,
        (__bridge NSString *)kAXWindowMiniaturizedNotification,(__bridge NSString *)kAXWindowDeminiaturizedNotification])
        AXObserverAddNotification(observer,(AXUIElementRef)element,(__bridge CFStringRef)name,(void *)(intptr_t)pid);
}
static void notification(AXObserverRef observer,AXUIElementRef element,CFStringRef name,void *context) {
    (void)observer;
    if(CFEqual(name,kAXFocusedWindowChangedNotification))pendingEvents|=RIBBON_EVENT_FOCUS;
    else if(CFEqual(name,kAXWindowMovedNotification)||CFEqual(name,kAXWindowResizedNotification))pendingEvents|=RIBBON_EVENT_GEOMETRY;
    else {
        pendingEvents|=RIBBON_EVENT_WINDOWS;
        if(CFEqual(name,kAXCreatedNotification))ribbon_watch_ax_element(element,(int)(intptr_t)context);
        if(CFEqual(name,kAXUIElementDestroyedNotification)&&watchedElements) {
            NSDictionary *known=(__bridge NSDictionary *)CFDictionaryGetValue(watchedElements,element);
            if(known) {
                if(!closedWindows)closedWindows=[NSMutableDictionary dictionary];
                closedWindows[known[@"wid"]]=known[@"pid"];
                CFDictionaryRemoveValue(watchedElements,element);
            }
        }
    }
}
static void initialize(void) {
    if(observers)return;
    observers=[NSMutableDictionary dictionary];workspaceObservers=[NSMutableArray array];
    NSNotificationCenter *center=NSWorkspace.sharedWorkspace.notificationCenter;
    for(NSString *name in @[NSWorkspaceDidLaunchApplicationNotification,NSWorkspaceDidTerminateApplicationNotification,
        NSWorkspaceDidActivateApplicationNotification,NSWorkspaceActiveSpaceDidChangeNotification]) {
        id token=[center addObserverForName:name object:nil queue:NSOperationQueue.mainQueue usingBlock:^(NSNotification *note) {
            if([note.name isEqual:NSWorkspaceDidActivateApplicationNotification])pendingEvents|=RIBBON_EVENT_FOCUS;
            else if([note.name isEqual:NSWorkspaceActiveSpaceDidChangeNotification])pendingEvents|=RIBBON_EVENT_WINDOWS;
            else pendingEvents|=RIBBON_EVENT_APPS|RIBBON_EVENT_WINDOWS;
        }];
        [workspaceObservers addObject:token];
    }
}
int ribbon_watch_application(int pid) { @autoreleasepool {
    initialize();if(observers[@(pid)])return 1;
    AXObserverRef observer=NULL;
    if(AXObserverCreate(pid,notification,&observer))return 0;
    AXUIElementRef app=AXUIElementCreateApplication(pid);
    AXUIElementSetMessagingTimeout(app,0.05);
    BOOL subscribed=NO;
    for(NSString *name in @[(__bridge NSString *)kAXCreatedNotification,(__bridge NSString *)kAXFocusedWindowChangedNotification]) {
        AXError error=AXObserverAddNotification(observer,app,(__bridge CFStringRef)name,(void *)(intptr_t)pid);
        if(!error||error==kAXErrorNotificationAlreadyRegistered)subscribed=YES;
    }
    if(subscribed) {
        observers[@(pid)]=(__bridge id)observer;
        CFRunLoopAddSource(CFRunLoopGetMain(),AXObserverGetRunLoopSource(observer),kCFRunLoopDefaultMode);
        // Registration may succeed after initial inventory discovery. Attach
        // existing windows too, so a startup retry does not lose resize events.
        CFTypeRef windows=NULL;
        if(!AXUIElementCopyAttributeValue(app,kAXWindowsAttribute,&windows)&&windows&&CFGetTypeID(windows)==CFArrayGetTypeID())
            for(id window in (__bridge NSArray *)windows)ribbon_watch_ax_element((__bridge const void *)window,pid);
        if(windows)CFRelease(windows);
    }
    CFRelease(app);
    CFRelease(observer);return subscribed;
} }
void ribbon_unwatch_application(int pid) {
    AXObserverRef observer=(__bridge AXObserverRef)observers[@(pid)];
    if(observer)CFRunLoopRemoveSource(CFRunLoopGetMain(),AXObserverGetRunLoopSource(observer),kCFRunLoopDefaultMode);
    [observers removeObjectForKey:@(pid)];
    if(watchedElements) {
        NSDictionary *entries=(__bridge NSDictionary *)watchedElements;
        for(id element in entries.allKeys)if([entries[element][@"pid"] intValue]==pid)
            CFDictionaryRemoveValue(watchedElements,(__bridge const void *)element);
    }
}
size_t ribbon_take_closed_windows(RibbonClosedWindow *windows,size_t capacity,int refresh) {
    // A retained NSWindow can be closed/withdrawn without destroying its AX
    // element or WindowServer surface. Reconcile successful AX window lists
    // for the already-authorized observed PIDs; a failed query proves nothing.
    double now=NSProcessInfo.processInfo.systemUptime;
    if(refresh&&watchedElements&&now-lastMembershipCheck>=1) {
        lastMembershipCheck=now;
        for(NSNumber *pid in observers.allKeys) {
            AXUIElementRef app=AXUIElementCreateApplication(pid.intValue);
            AXUIElementSetMessagingTimeout(app,0.05);CFTypeRef members=NULL;
            AXError error=AXUIElementCopyAttributeValue(app,kAXWindowsAttribute,&members);CFRelease(app);
            if(!error&&members&&CFGetTypeID(members)==CFArrayGetTypeID()) {
                NSMutableSet *ids=[NSMutableSet set];
                for(id member in (__bridge NSArray *)members) {
                    uint32_t wid=ribbon_ax_window_id((__bridge const void *)member);
                    if(wid)[ids addObject:@(wid)];
                }
                NSDictionary *entries=(__bridge NSDictionary *)watchedElements;
                for(id element in entries.allKeys) {
                    NSDictionary *known=entries[element];
                    if([known[@"pid"] isEqual:pid]&&![ids containsObject:known[@"wid"]]) {
                        if(!closedWindows)closedWindows=[NSMutableDictionary dictionary];
                        closedWindows[known[@"wid"]]=pid;
                        CFDictionaryRemoveValue(watchedElements,(__bridge const void *)element);
                    }
                }
            }
            if(members)CFRelease(members);
        }
    }
    size_t count=0;
    for(NSNumber *wid in closedWindows.allKeys) {
        if(count==capacity)break;
        windows[count++]=(RibbonClosedWindow){wid.unsignedIntValue,closedWindows[wid].intValue};
        [closedWindows removeObjectForKey:wid];
    }
    return count;
}
uint32_t ribbon_events(void) { @autoreleasepool {
    initialize();
    // A bounded batch per Rust frame avoids recursive relayout on event bursts.
    for(int i=0;i<16;i++)if(CFRunLoopRunInMode(kCFRunLoopDefaultMode,0,true)!=kCFRunLoopRunHandledSource)break;
    uint32_t events=pendingEvents;pendingEvents=0;return events;
} }
void ribbon_stop_observing(void) {
    for(NSNumber *pid in observers.allKeys)ribbon_unwatch_application(pid.intValue);
    for(id token in workspaceObservers)[NSWorkspace.sharedWorkspace.notificationCenter removeObserver:token];
    observers=nil;workspaceObservers=nil;pendingEvents=0;lastMembershipCheck=0;
    if(watchedElements)CFRelease(watchedElements);watchedElements=NULL;closedWindows=nil;
}
