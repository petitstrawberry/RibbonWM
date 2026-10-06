// Notification transport only. Rust chooses eligible apps and owns layout policy.
#import <Cocoa/Cocoa.h>
#import <ApplicationServices/ApplicationServices.h>
#include "bridge.h"
#include <dlfcn.h>
static NSMutableDictionary<NSNumber *,id> *observers;
static NSMutableArray *workspaceObservers;
static uint32_t pendingEvents;
static CFMutableDictionaryRef watchedElements;
static NSMutableDictionary<NSNumber *,NSDictionary *> *closedWindows;
static double lastMembershipCheck;
static AXObserverRef dockObserver;
static AXUIElementRef dockElement;
static pid_t dockPID;
static int overviewActive;
static int overviewSignal;
static double lastOverviewPoll,overviewBegan;
static CFMachPortRef mouseTap;
static CFRunLoopSourceRef mouseSource;
static RibbonMouseState mouseState;
static double lastMouseTapAttempt;
static void overviewConnectionEvent(uint32_t type,void *data,size_t size,void *context,int cid) {
    (void)data;(void)size;(void)context;(void)cid;
    if(type==1204) {__atomic_store_n(&overviewSignal,1,__ATOMIC_RELEASE);__atomic_store_n(&overviewActive,1,__ATOMIC_RELEASE);}
}
static void observeOverviewConnection(void) {
    static dispatch_once_t once;
    dispatch_once(&once,^{
        void *sky=dlopen("/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight",RTLD_NOW);
        int (*connection)(void)=dlsym(sky,"SLSMainConnectionID");
        CGError (*observe)(int,void (*)(uint32_t,void *,size_t,void *,int),uint32_t,void *)=dlsym(sky,"SLSRegisterConnectionNotifyProc");
        if(connection&&observe)observe(connection(),overviewConnectionEvent,1204,NULL);
    });
}
static CGEventRef mouseEvent(CGEventTapProxy proxy,CGEventType type,CGEventRef event,void *context) {
    (void)proxy;(void)context;
    if(type==kCGEventTapDisabledByTimeout||type==kCGEventTapDisabledByUserInput) {
        if(mouseTap)CGEventTapEnable(mouseTap,true);return event;
    }
    if(type==kCGEventLeftMouseDown) {
        CGPoint p=CGEventGetLocation(event);
        mouseState=(RibbonMouseState){p.x,p.y,(uint32_t)CGEventGetIntegerValueField(event,kCGMouseEventWindowUnderMousePointer),1,0};
    } else if(type==kCGEventLeftMouseDragged)mouseState.dragged=1;
    else if(type==kCGEventLeftMouseUp)mouseState.down=mouseState.dragged=0;
    return event;
}
RibbonMouseState ribbon_mouse_state(void) {
    double now=NSProcessInfo.processInfo.systemUptime;
    if(!mouseTap&&now-lastMouseTapAttempt>=1) {
        lastMouseTapAttempt=now;
        mouseTap=CGEventTapCreate(kCGHIDEventTap,kCGHeadInsertEventTap,kCGEventTapOptionListenOnly,
            CGEventMaskBit(kCGEventLeftMouseDown)|CGEventMaskBit(kCGEventLeftMouseDragged)|CGEventMaskBit(kCGEventLeftMouseUp),mouseEvent,NULL);
        if(mouseTap) {
            mouseSource=CFMachPortCreateRunLoopSource(NULL,mouseTap,0);
            if(!mouseSource){CFRelease(mouseTap);mouseTap=NULL;return mouseState;}
            CFRunLoopAddSource(CFRunLoopGetMain(),mouseSource,kCFRunLoopDefaultMode);CGEventTapEnable(mouseTap,true);
        }
    }
    if(!ribbon_left_mouse_down())mouseState.down=mouseState.dragged=0;
    return mouseState;
}
static void overviewNotification(AXObserverRef observer,AXUIElementRef element,CFStringRef name,void *context) {
    (void)observer;(void)element;(void)context;
    BOOL active=!CFEqual(name,CFSTR("AXExposeExit"));
    __atomic_store_n(&overviewActive,active,__ATOMIC_RELEASE);
    if(active)overviewBegan=NSProcessInfo.processInfo.systemUptime;
}
int ribbon_mission_control_active(void) { @autoreleasepool {
    observeOverviewConnection();
    // Same Dock notifications and layer-18 fallback used by yabai. Observe only
    // Dock; never inspect another application's AX hierarchy for this state.
    NSRunningApplication *dock=[NSRunningApplication runningApplicationsWithBundleIdentifier:@"com.apple.dock"].firstObject;
    if(dock.processIdentifier!=dockPID) {
        if(dockObserver){CFRunLoopRemoveSource(CFRunLoopGetMain(),AXObserverGetRunLoopSource(dockObserver),kCFRunLoopDefaultMode);CFRelease(dockObserver);dockObserver=NULL;}
        if(dockElement){CFRelease(dockElement);dockElement=NULL;}
        dockPID=dock.processIdentifier;
        if(dockPID>0&&!AXObserverCreate(dockPID,overviewNotification,&dockObserver)) {
            dockElement=AXUIElementCreateApplication(dockPID);
            for(NSString *name in @[@"AXExposeShowAllWindows",@"AXExposeShowFrontWindows",@"AXExposeShowDesktop",@"AXExposeExit"])
                AXObserverAddNotification(dockObserver,dockElement,(__bridge CFStringRef)name,NULL);
            CFRunLoopAddSource(CFRunLoopGetMain(),AXObserverGetRunLoopSource(dockObserver),kCFRunLoopDefaultMode);
        }
    }
    double now=NSProcessInfo.processInfo.systemUptime;
    if(__atomic_exchange_n(&overviewSignal,0,__ATOMIC_ACQ_REL))overviewBegan=now;
    if(now-lastOverviewPoll>=0.1) {
        lastOverviewPoll=now;BOOL found=NO;
        CFArrayRef rows=CGWindowListCopyWindowInfo(kCGWindowListOptionOnScreenOnly,kCGNullWindowID);
        for(NSDictionary *row in (__bridge NSArray *)rows)
            if([row[(id)kCGWindowOwnerPID] intValue]==dockPID&&[row[(id)kCGWindowLayer] intValue]==18&&!row[(id)kCGWindowName]){found=YES;break;}
        if(rows)CFRelease(rows);
        if(found||now-overviewBegan>0.4)__atomic_store_n(&overviewActive,found,__ATOMIC_RELEASE);
    }
    return __atomic_load_n(&overviewActive,__ATOMIC_ACQUIRE);
} }
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
                closedWindows[known[@"wid"]]=@{@"pid":known[@"pid"],@"withdrawn":@NO};
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
                        if(!closedWindows[known[@"wid"]])
                            closedWindows[known[@"wid"]]=@{@"pid":pid,@"withdrawn":@YES};
                    }
                }
            }
            if(members)CFRelease(members);
        }
    }
    size_t count=0;
    for(NSNumber *wid in closedWindows.allKeys) {
        if(count==capacity)break;
        NSDictionary *record=closedWindows[wid];
        windows[count++]=(RibbonClosedWindow){wid.unsignedIntValue,[record[@"pid"] intValue],[record[@"withdrawn"] intValue]};
        [closedWindows removeObjectForKey:wid];
    }
    return count;
}
void ribbon_forget_watched_window(uint32_t wid) {
    if(!watchedElements)return;
    NSDictionary *entries=(__bridge NSDictionary *)watchedElements;
    for(id element in entries.allKeys)
        if([entries[element][@"wid"] unsignedIntValue]==wid)
            CFDictionaryRemoveValue(watchedElements,(__bridge const void *)element);
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
    if(dockObserver){CFRunLoopRemoveSource(CFRunLoopGetMain(),AXObserverGetRunLoopSource(dockObserver),kCFRunLoopDefaultMode);CFRelease(dockObserver);dockObserver=NULL;}
    if(dockElement)CFRelease(dockElement);dockElement=NULL;dockPID=0;
    __atomic_store_n(&overviewActive,0,__ATOMIC_RELEASE);__atomic_store_n(&overviewSignal,0,__ATOMIC_RELEASE);
    lastOverviewPoll=overviewBegan=0;
    if(mouseTap){CGEventTapEnable(mouseTap,false);CFMachPortInvalidate(mouseTap);CFRelease(mouseTap);mouseTap=NULL;}
    if(mouseSource){CFRunLoopRemoveSource(CFRunLoopGetMain(),mouseSource,kCFRunLoopDefaultMode);CFRelease(mouseSource);mouseSource=NULL;}
    mouseState=(RibbonMouseState){0};lastMouseTapAttempt=0;
}
