// Minimal Dock payload: transform and clipping only. No symbol-pattern scanning.
#import "skylight.h"
#include <sys/socket.h>
#include <sys/un.h>
#include <sys/stat.h>
#include <sys/select.h>
#include <unistd.h>
#include <pthread.h>
#include <math.h>
#include <limits.h>

@interface RibbonSavedWindowV4 : NSObject
@property uint32_t wid;
@property CGRect bounds;
@property CGRect nativeBounds;
@property CGAffineTransform transform;
@property CFTypeRef clip;
@property pid_t pid;
@property uint32_t root;
@end
@implementation RibbonSavedWindowV4
@end

static SkyLight sky;
static NSMutableDictionary<NSNumber *, RibbonSavedWindowV4 *> *saved;
static NSMutableDictionary<NSNumber *,NSDictionary *> *stickySaved;
static RibbonStickyAPI stickyAPI;
static bool hasStickyAPI;
static NSUInteger controlledCount(void) {
    NSMutableSet *ids=[NSMutableSet setWithArray:saved.allKeys];
    [ids addObjectsFromArray:stickySaved.allKeys];return ids.count;
}
static double lastUpdate;
static NSString *controller;
static NSString *buildName;
static CGError (*getOwner)(int,uint32_t,int *);
static CGError (*connectionPID)(int,pid_t *);
static pid_t ownerPID(uint32_t wid) {
    int cid=0;pid_t pid=0;
    return getOwner(sky.connection(),wid,&cid)||connectionPID(cid,&pid)?0:pid;
}
static BOOL validSurfaceBounds(CGRect b) {
    return isfinite(b.origin.x)&&isfinite(b.origin.y)&&isfinite(b.size.width)&&isfinite(b.size.height)
        &&b.size.width>0&&b.size.height>0;
}

static unsigned restoreWindows(NSSet<NSNumber *> *retained) {
    unsigned failed=0;
    for (RibbonSavedWindowV4 *w in saved.allValues) {
        if ([retained containsObject:@(w.wid)]) continue;
        if (ownerPID(w.wid)==w.pid) {
            CGRect b;
            CGError eb=sky.getBounds(sky.connection(),w.wid,&b);
            // A withdrawn attached surface may keep its owner while reporting
            // a null rectangle. Its last finite native bounds remain the only
            // usable anchor; never feed the null sentinel into a transform.
            if(eb||!validSurfaceBounds(b))b=w.nativeBounds;
            if(!validSurfaceBounds(b)){failed++;NSLog(@"[RibbonWM] restore bounds unavailable wid=%u root=%u error=%d",w.wid,w.root,eb);continue;}
            // A lease may have started with a stale translated surface. The
            // current owner bounds are the native desktop position; replaying
            // that old absolute transform can send a moved window off-screen.
            CGAffineTransform native=CGAffineTransformMakeTranslation(-b.origin.x,-b.origin.y);
            CGError et=sky.setTransform(sky.connection(),w.wid,native);
            CFTypeRef clip=w.clip,replacement=NULL;
            if(fabs(b.size.width-w.bounds.size.width)>2||fabs(b.size.height-w.bounds.size.height)>2) {
                CGRect full=CGRectMake(0,0,b.size.width,b.size.height);
                if(sky.newRegion(&full,&replacement)){failed++;continue;}clip=replacement;
            }
            CGError ec=sky.setClip(sky.connection(),w.wid,clip);
            if(replacement)sky.releaseRegion(replacement);
            if(et||ec) {failed++;NSLog(@"[RibbonWM] restore wid=%u transform=%d clip=%d",w.wid,et,ec);continue;}
        }
        sky.releaseRegion(w.clip);[saved removeObjectForKey:@(w.wid)];
    }
    if(controlledCount()==0)controller=nil;
    return failed;
}
static unsigned restoreSticky(NSSet<NSNumber *> *retained) {
    unsigned failed=0;
    for(NSNumber *wid in stickySaved.allKeys) {
        if([retained containsObject:wid])continue;
        NSDictionary *old=stickySaved[wid];
        if(ownerPID(wid.unsignedIntValue)==[old[@"pid"] intValue]) {
            uint64_t mask=UINT64_C(1)<<11;
            CGError e=[old[@"original"] boolValue]?stickyAPI.set(sky.connection(),wid.unsignedIntValue,&mask,64):stickyAPI.clear(sky.connection(),wid.unsignedIntValue,&mask,64);
            bool actual=false;
            if(e||!readSticky(&stickyAPI,sky.connection(),wid.unsignedIntValue,&actual)||actual!=[old[@"original"] boolValue]){failed++;continue;}
        }
        [stickySaved removeObjectForKey:wid];
    }
    if(controlledCount()==0)controller=nil;
    return failed;
}
static unsigned restoreAll(void) {return restoreWindows([NSSet set])+restoreSticky([NSSet set]);}
static void forgetWindow(uint32_t wid) {
    RibbonSavedWindowV4 *w=saved[@(wid)];
    if(w){sky.releaseRegion(w.clip);[saved removeObjectForKey:@(wid)];}
}

static NSDictionary *error(NSString *message) { return @{@"ok":@NO, @"error":message}; }

static bool number(id value, double *out) {
    if (![value isKindOfClass:NSNumber.class]) return false;
    *out = [value doubleValue];
    return isfinite(*out) && fabs(*out) <= 1000000;
}

static bool rect(id value,CGRect *out) {
    double x,y,width,height;
    if(![value isKindOfClass:NSDictionary.class]||!number(value[@"x"],&x)||!number(value[@"y"],&y)||
        !number(value[@"width"],&width)||!number(value[@"height"],&height)||width<=0||height<=0)return false;
    *out=CGRectMake(x,y,width,height);return true;
}
static bool identifier(id value,double limit,double *out) {
    if(![value isKindOfClass:NSNumber.class]||CFGetTypeID((__bridge CFTypeRef)value)==CFBooleanGetTypeID())return false;
    *out=[value doubleValue];
    return isfinite(*out)&&*out>=1&&*out<=limit&&floor(*out)==*out;
}
static bool stickyDescriptor(id u) {
    double wid=0,pid=0;
    return [u isKindOfClass:NSDictionary.class]&&identifier(u[@"wid"],UINT32_MAX,&wid)&&
        identifier(u[@"pid"],INT_MAX,&pid)&&
        [u[@"enabled"] isKindOfClass:NSNumber.class]&&CFGetTypeID((__bridge CFTypeRef)u[@"enabled"])==CFBooleanGetTypeID();
}
// WindowServer's explicit parent relationship includes AppKit child surfaces
// and macOS capture indicators. PID equality alone is not a relationship.
static NSDictionary<NSNumber *,NSValue *> *familyBounds(uint32_t root,pid_t pid) {
    static CFArrayRef (*associated)(int,uint32_t);
    static CFTypeRef (*query)(int,CFArrayRef,int),(*iterator)(CFTypeRef);
    static bool (*advance)(CFTypeRef);
    static uint32_t (*getID)(CFTypeRef),(*parentID)(CFTypeRef);
    static dispatch_once_t once;
    dispatch_once(&once,^{
        void *h=dlopen("/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight",RTLD_NOW);
        associated=dlsym(h,"SLSCopyAssociatedWindows");query=dlsym(h,"SLSWindowQueryWindows");
        iterator=dlsym(h,"SLSWindowQueryResultCopyWindows");advance=dlsym(h,"SLSWindowIteratorAdvance");
        getID=dlsym(h,"SLSWindowIteratorGetWindowID");parentID=dlsym(h,"SLSWindowIteratorGetParentID");
    });
    if(!associated||!query||!iterator||!advance||!getID||!parentID)return nil;
    CFArrayRef ids=associated(sky.connection(),root);
    if(!ids)return @{};
    if(CFArrayGetCount(ids)>512){CFRelease(ids);return nil;}
    CFTypeRef q=query(sky.connection(),ids,(int)CFArrayGetCount(ids)),i=q?iterator(q):NULL;
    NSMutableDictionary *parents=[NSMutableDictionary dictionary],*result=[NSMutableDictionary dictionary];
    while(i&&advance(i)) {
        uint32_t child=getID(i);
        if(child&&child!=root&&ownerPID(child)==pid)parents[@(child)]=@(parentID(i));
    }
    if(i)CFRelease(i);if(q)CFRelease(q);CFRelease(ids);
    NSMutableSet *reached=[NSMutableSet setWithObject:@(root)];BOOL added=YES;
    while(added) {
        added=NO;
        for(NSNumber *child in parents.allKeys) {
            if([reached containsObject:child]||![reached containsObject:parents[child]])continue;
            CGRect b;
            if(sky.getBounds(sky.connection(),child.unsignedIntValue,&b)||
                !isfinite(b.origin.x)||!isfinite(b.origin.y)||!isfinite(b.size.width)||!isfinite(b.size.height)||
                b.size.width<=0||b.size.height<=0)continue;
            result[child]=[NSValue valueWithRect:b];[reached addObject:child];added=YES;
        }
    }
    return result;
}
static NSDictionary *rectObject(CGRect r) {
    return @{@"x":@(r.origin.x),@"y":@(r.origin.y),@"width":@(r.size.width),@"height":@(r.size.height)};
}
static NSArray *expandFamilies(NSArray *roots) {
    NSMutableArray *updates=[NSMutableArray array];NSMutableSet *rootIDs=[NSMutableSet set];
    for(NSDictionary *u in roots)[rootIDs addObject:u[@"wid"]];
    for(NSDictionary *u in roots) {
        uint32_t root=[u[@"wid"] unsignedIntValue];pid_t pid=ownerPID(root);
        NSMutableDictionary *main=[u mutableCopy];main[@"group_root"]=@(root);[updates addObject:main];
        if(!pid||(u[@"pid"]&&pid!=[u[@"pid"] intValue]))continue;
        NSDictionary *family=familyBounds(root,pid);if(!family)return nil;
        if(!family.count)continue;
        CGRect physical,shown;CGAffineTransform t;
        if(sky.getBounds(sky.connection(),root,&physical)||!validSurfaceBounds(physical))return nil;
        rect(u[@"frame"],&shown);
        if(u[@"viewport"]&&!u[@"drag_frame"]) {
            if(sky.getTransform(sky.connection(),root,&t))return nil;
            shown.origin=CGPointMake(-t.tx,-t.ty);
        } else if(u[@"drag_frame"])rect(u[@"drag_frame"],&shown);
        CGRect viewport;BOOL hasViewport=rect(u[@"clip_viewport"]?:u[@"viewport"],&viewport);
        for(NSNumber *child in family) {
            if([rootIDs containsObject:child])continue;
            CGRect b=[family[child] rectValue];
            CGRect f=CGRectMake(shown.origin.x+b.origin.x-physical.origin.x,
                shown.origin.y+b.origin.y-physical.origin.y,b.size.width,b.size.height);
            CGRect c=hasViewport?CGRectIntersection(f,viewport):f;
            id clip=(u[@"clip"]==NSNull.null||CGRectIsNull(c)||CGRectIsEmpty(c))?NSNull.null:rectObject(c);
            NSMutableDictionary *derived=[@{@"wid":child,@"pid":@(pid),@"group_root":@(root),
                @"frame":rectObject(f),@"clip":clip} mutableCopy];
            if(u[@"viewport"]) {derived[@"viewport"]=u[@"viewport"];derived[@"drag_frame"]=rectObject(f);}
            if(hasViewport)derived[@"clip_viewport"]=rectObject(viewport);
            [updates addObject:derived];if(updates.count>512)return nil;
        }
    }
    return updates;
}
static NSDictionary *setSticky(NSDictionary *u,NSString *session) {
    if(!hasStickyAPI)return error(@"Sticky API unavailable");
    if(!stickyDescriptor(u))return error(@"Invalid sticky descriptor");
    uint32_t wid=[u[@"wid"] unsignedIntValue];pid_t pid=[u[@"pid"] intValue];
    if(ownerPID(wid)!=pid)return error(@"Sticky owner changed");
    NSDictionary *old=stickySaved[@(wid)];
    if(old&&[old[@"pid"] intValue]!=pid){[stickySaved removeObjectForKey:@(wid)];old=nil;}
    bool original=false;
    if(!old) {
        if(!saved[@(wid)]&&controlledCount()>=128)return error(@"At most 128 total windows");
        if(!readSticky(&stickyAPI,sky.connection(),wid,&original))return error(@"Cannot snapshot sticky tag");
        old=@{@"pid":@(pid),@"original":@(original)};stickySaved[@(wid)]=old;
    }
    controller=session;lastUpdate=NSProcessInfo.processInfo.systemUptime;
    uint64_t mask=UINT64_C(1)<<11;BOOL enabled=[u[@"enabled"] boolValue];
    bool actual=false;
    if(readSticky(&stickyAPI,sky.connection(),wid,&actual)&&actual==enabled)return @{@"ok":@YES};
    CGError e=enabled?stickyAPI.set(sky.connection(),wid,&mask,64):stickyAPI.clear(sky.connection(),wid,&mask,64);
    if(e||!readSticky(&stickyAPI,sky.connection(),wid,&actual)||actual!=enabled)return error(@"Sticky tag was not accepted; snapshot retained for restoration");
    return @{@"ok":@YES};
}
static NSDictionary *frame(NSDictionary *r) {
    NSArray *updates=r[@"updates"],*stickies=r[@"stickies"]?:@[];
    if(![stickies isKindOfClass:NSArray.class]||stickies.count>128||(stickies.count&&!hasStickyAPI))return error(@"Invalid or unavailable sticky lease");
    NSMutableSet *stickyIDs=[NSMutableSet set];
    for(id u in stickies) {
        if(!stickyDescriptor(u)||[stickyIDs containsObject:u[@"wid"]])return error(@"Invalid or duplicate sticky lease");
        [stickyIDs addObject:u[@"wid"]];
    }
    if(![updates isKindOfClass:NSArray.class]||updates.count>128)return error(@"Expected at most 128 updates");
    NSMutableSet *ids=[NSMutableSet set];
    for(id u in updates) {
        if(![u isKindOfClass:NSDictionary.class])return error(@"Invalid update");
        double v=0;
        CGRect f,c;
        if(!identifier(u[@"wid"],UINT32_MAX,&v)||[ids containsObject:@((uint32_t)v)]||!rect(u[@"frame"],&f)||
            (u[@"clip"]!=NSNull.null&&!rect(u[@"clip"],&c)))return error(@"Invalid window, frame, clip, or duplicate ID");
        double pid=0;
        if(u[@"pid"]&&!identifier(u[@"pid"],INT_MAX,&pid))return error(@"Invalid expected owner PID");
        CGRect viewport;
        if(u[@"viewport"]&&!rect(u[@"viewport"],&viewport))return error(@"Invalid interactive viewport");
        if(u[@"clip_viewport"]&&!rect(u[@"clip_viewport"],&viewport))return error(@"Invalid group viewport");
        CGRect dragFrame;
        if(u[@"drag_frame"]&&(!u[@"viewport"]||!rect(u[@"drag_frame"],&dragFrame)))return error(@"Invalid pointer drag frame");
        [ids addObject:@((uint32_t)v)];
    }
    updates=expandFamilies(updates);
    if(!updates)return error(@"Cannot resolve bounded owner window families");
    for(NSDictionary *u in updates)[ids addObject:u[@"wid"]];
    NSMutableSet *allIDs=[ids mutableCopy];[allIDs unionSet:stickyIDs];
    if(allIDs.count>512)return error(@"At most 512 surfaces including children");
    if(restoreSticky(stickyIDs))return error(@"Could not restore removed sticky lease");
    for(NSDictionary *u in stickies) {
        if(ownerPID([u[@"wid"] unsignedIntValue])!=[u[@"pid"] intValue])continue;
        NSDictionary *reply=setSticky(u,r[@"session"]);if(![reply[@"ok"] boolValue])return reply;
    }
    if(restoreWindows(ids))return error(@"Could not restore windows removed from the frame");
    controller=r[@"session"];
    for(NSDictionary *u in updates) {
        uint32_t wid=[u[@"wid"] unsignedIntValue];CGRect f,c;
        rect(u[@"frame"],&f);
        RibbonSavedWindowV4 *w=saved[@(wid)];pid_t pid=ownerPID(wid);
        // Closing/reused IDs are normal lifecycle events. Release their stale
        // snapshots without restoring or touching the new owner's window.
        if(!pid||(w&&w.pid!=pid)) {forgetWindow(wid);continue;}
        if(u[@"pid"]&&[u[@"pid"] intValue]!=pid) {
            if(w){lastUpdate=NSProcessInfo.processInfo.systemUptime;return error(@"Expected owner changed for a controlled window");}
            continue;
        }
        if(!w) {
            CGRect b;CGAffineTransform t;CFTypeRef clip=NULL;
            CGError eb=sky.getBounds(sky.connection(),wid,&b),et=sky.getTransform(sky.connection(),wid,&t),ec=sky.copyClip(sky.connection(),wid,&clip);
            if(eb||et||ec||!clip||!validSurfaceBounds(b)) {
                if(clip)sky.releaseRegion(clip);
                if(ownerPID(wid)!=pid)continue;
                return error(@"Cannot save original window state");
            }
            // A normal app can have a translation offset between its logical
            // bounds and presentation (observed with Chrome). Preserve that
            // translation verbatim; only scaled/rotated/sheared input is unsupported.
            if(fabs(t.a-1)>1e-6||fabs(t.b)>1e-6||fabs(t.c)>1e-6||fabs(t.d-1)>1e-6||!isfinite(t.tx)||!isfinite(t.ty)) {
                sky.releaseRegion(clip);return error(@"Window already has a custom transform");
            }
            w=[RibbonSavedWindowV4 new];w.wid=wid;w.bounds=b;w.nativeBounds=b;w.transform=t;w.clip=clip;w.pid=pid;
            w.root=[u[@"group_root"] unsignedIntValue];saved[@(wid)]=w;
        }
        CGRect observed;
        if(!sky.getBounds(sky.connection(),wid,&observed)&&validSurfaceBounds(observed))w.nativeBounds=observed;
        CGRect local=CGRectZero;
        BOOL interactive=u[@"viewport"]!=nil;
        BOOL pointerDrag=u[@"drag_frame"]!=nil;
        if(interactive) {
            // Rust decides when to suspend placement. Read the owner's current
            // surface; clip it to the leased monitor without moving it back.
            CGRect bounds,viewport;CGAffineTransform current;
            if(sky.getBounds(sky.connection(),wid,&bounds)||sky.getTransform(sky.connection(),wid,&current))return error(@"Cannot read interactive surface");
            if(fabs(current.a-1)>1e-6||fabs(current.b)>1e-6||fabs(current.c)>1e-6||fabs(current.d-1)>1e-6)return error(@"Unsupported interactive transform");
            rect(u[@"viewport"],&viewport);
            CGRect shown=CGRectMake(-current.tx,-current.ty,bounds.size.width,bounds.size.height);
            if(pointerDrag) {
                CGRect desired;rect(u[@"drag_frame"],&desired);
                shown.origin=desired.origin;f.origin=desired.origin;
            }
            CGRect visible=CGRectIntersection(shown,viewport);
            if(!CGRectIsNull(visible)&&!CGRectIsEmpty(visible))local=CGRectMake(visible.origin.x-shown.origin.x,visible.origin.y-shown.origin.y,visible.size.width,visible.size.height);
        } else if(u[@"clip"]!=NSNull.null) {rect(u[@"clip"],&c);local=CGRectMake(c.origin.x-f.origin.x,c.origin.y-f.origin.y,c.size.width,c.size.height);}
        CFTypeRef region=NULL;CGError er=sky.newRegion(&local,&region);
        CGError ec=er?er:sky.setClip(sky.connection(),wid,region);
        CGError et=ec?ec:(interactive&&!pointerDrag?0:sky.setTransform(sky.connection(),wid,CGAffineTransformMakeTranslation(-f.origin.x,-f.origin.y)));
        if(region)sky.releaseRegion(region);
        if(ec||et) {
            if(ownerPID(wid)!=pid){forgetWindow(wid);continue;}
            // Preserve snapshots so the client can restore AX geometry before
            // reset restores compositor state. The watchdog remains armed.
            lastUpdate=NSProcessInfo.processInfo.systemUptime;
            return error([NSString stringWithFormat:@"Apply failed: clip=%d transform=%d",ec,et]);
        }
    }
    lastUpdate=NSProcessInfo.processInfo.systemUptime;
    if(controlledCount()==0)controller=nil;
    return @{@"ok":@YES,@"controlled":@(controlledCount())};
}
static NSDictionary *overview(void) {
    for(RibbonSavedWindowV4 *w in saved.allValues) {
        if(ownerPID(w.wid)!=w.pid){forgetWindow(w.wid);continue;}
        CGRect b;CGError eb=sky.getBounds(sky.connection(),w.wid,&b);
        if(eb||!isfinite(b.size.width)||!isfinite(b.size.height)||b.size.width<=0||b.size.height<=0)return error(@"Cannot read overview surface");
        CGRect full=CGRectMake(0,0,b.size.width,b.size.height);CFTypeRef region=NULL;
        CGError er=sky.newRegion(&full,&region);
        CGError ec=er?er:sky.setClip(sky.connection(),w.wid,region);
        if(region)sky.releaseRegion(region);
        CGAffineTransform native=CGAffineTransformMakeTranslation(-b.origin.x,-b.origin.y);
        CGError et=ec?ec:sky.setTransform(sky.connection(),w.wid,native);
        if(ec||et)return error(@"Cannot expose full window");
    }
    lastUpdate=NSProcessInfo.processInfo.systemUptime;
    return @{@"ok":@YES};
}
static NSDictionary *releaseFrames(NSDictionary *r,BOOL all) {
    NSArray *updates=r[@"updates"];
    if(![updates isKindOfClass:NSArray.class]||updates.count>128)return error(@"Invalid release updates");
    NSMutableSet *ids=[NSMutableSet set];
    for(id u in updates) {
        double wid=0,pid=0;CGRect f;
        if(![u isKindOfClass:NSDictionary.class]||!identifier(u[@"wid"],UINT32_MAX,&wid)||
           !identifier(u[@"pid"],INT_MAX,&pid)||!rect(u[@"frame"],&f)||[ids containsObject:@((uint32_t)wid)])return error(@"Invalid release descriptor");
        RibbonSavedWindowV4 *old=saved[@((uint32_t)wid)];
        if(old&&old.pid!=(int)pid)return error(@"Release requires a matching lease");
        [ids addObject:@((uint32_t)wid)];
    }
    updates=expandFamilies(updates);
    if(!updates)return error(@"Cannot resolve released window families");
    // Commit the usable desktop frame rather than returning to an obsolete
    // startup transform/clip. Rust has settled the corresponding AX geometry.
    NSMutableSet *finished=[NSMutableSet set];
    for(NSDictionary *u in updates) {
        uint32_t wid=[u[@"wid"] unsignedIntValue];pid_t pid=[u[@"pid"] intValue];CGRect f;
        if(!saved[@(wid)])continue;
        rect(u[@"frame"],&f);
        if(ownerPID(wid)!=pid){forgetWindow(wid);continue;}
        CGRect b;CGError eb=sky.getBounds(sky.connection(),wid,&b);
        if(eb||!isfinite(b.size.width)||!isfinite(b.size.height)||b.size.width<=0||b.size.height<=0)return error(@"Cannot read released surface");
        CGRect full=CGRectMake(0,0,b.size.width,b.size.height);CFTypeRef region=NULL;
        CGError er=sky.newRegion(&full,&region),ec=er?er:sky.setClip(sky.connection(),wid,region);
        if(region)sky.releaseRegion(region);
        CGError et=ec?ec:sky.setTransform(sky.connection(),wid,CGAffineTransformMakeTranslation(-f.origin.x,-f.origin.y));
        if(ec||et)return error(@"Cannot release full window");
        [finished addObject:@(wid)];
    }
    for(NSNumber *wid in finished)forgetWindow(wid.unsignedIntValue);
    unsigned failed=all?restoreAll():0;lastUpdate=NSProcessInfo.processInfo.systemUptime;
    if(!controlledCount())controller=nil;
    return failed?error(@"Remaining restoration failed"):@{@"ok":@YES};
}
static NSDictionary *finish(NSDictionary *r) {return releaseFrames(r,YES);}
static NSDictionary *handle(id r) {
    if(![r isKindOfClass:NSDictionary.class])return error(@"Expected JSON object");
    if([r[@"op"] isEqual:@"diagnostics"]) {
        NSMutableArray *leases=[NSMutableArray array];
        for(RibbonSavedWindowV4 *w in saved.allValues) {
            CGRect b;CGError eb=sky.getBounds(sky.connection(),w.wid,&b);
            [leases addObject:@{@"wid":@(w.wid),@"root":@(w.root),@"pid":@(w.pid),@"current_pid":@(ownerPID(w.wid)),
                @"bounds_error":@(eb),@"bounds_valid":@(!eb&&validSurfaceBounds(b)),
                @"native_bounds_valid":@(validSurfaceBounds(w.nativeBounds))}];
        }
        return @{@"ok":@YES,@"leases":leases,@"idle_seconds":@(NSProcessInfo.processInfo.systemUptime-lastUpdate)};
    }
    if([r[@"op"] isEqual:@"hello"])return @{@"ok":@YES,@"version":@2,@"capabilities":hasStickyAPI?@[@"sticky",@"interactive_clip",@"overview",@"finish",@"pointer_drag",@"window_groups"]:@[@"interactive_clip",@"overview",@"finish",@"pointer_drag",@"window_groups"],@"build":buildName?:@"",@"pid":@(getpid()),@"uid":@(getuid()),@"controlled":@(controlledCount())};
    NSString *session=r[@"session"];
    if(![session isKindOfClass:NSString.class]||session.length==0||session.length>128)return error(@"Invalid session");
    if(controller&&![controller isEqual:session])return error(@"Another controller holds the lease");
    if([r[@"op"] isEqual:@"reset"]) {unsigned failed=restoreAll();return failed?error(@"Restore failed; watchdog will retry"):@{@"ok":@YES};}
    if([r[@"op"] isEqual:@"frame"])return frame(r);
    if([r[@"op"] isEqual:@"sticky"])return setSticky(r[@"window"],session);
    if([r[@"op"] isEqual:@"overview"])return overview();
    if([r[@"op"] isEqual:@"heartbeat"]) {lastUpdate=NSProcessInfo.processInfo.systemUptime;return @{@"ok":@YES};}
    if([r[@"op"] isEqual:@"finish"])return finish(r);
    if([r[@"op"] isEqual:@"detach"])return releaseFrames(r,NO);
    return error(@"Unknown operation");
}

static void serve(int fd) {
    uid_t uid; gid_t gid;
    if (getpeereid(fd,&uid,&gid) || uid != getuid()) return;
    struct timeval timeout = {.tv_sec=0,.tv_usec=200000};
    setsockopt(fd,SOL_SOCKET,SO_RCVTIMEO,&timeout,sizeof(timeout));
    setsockopt(fd,SOL_SOCKET,SO_SNDTIMEO,&timeout,sizeof(timeout));
    int noSigpipe = 1; setsockopt(fd,SOL_SOCKET,SO_NOSIGPIPE,&noSigpipe,sizeof(noSigpipe));
    char bytes[65536]; size_t used = 0;
    double began=NSProcessInfo.processInfo.systemUptime;
    while (used<sizeof(bytes)) {
        if(NSProcessInfo.processInfo.systemUptime-began>0.2)return;
        ssize_t n = read(fd,bytes+used,sizeof(bytes)-used);
        if (n<=0) return;
        used+=(size_t)n;
        char *nl = memchr(bytes,'\n',used);
        if (!nl) continue;
        NSData *data = [NSData dataWithBytes:bytes length:(NSUInteger)(nl-bytes)];
        id request = [NSJSONSerialization JSONObjectWithData:data options:0 error:nil];
        NSDictionary *response = handle(request);
        NSMutableData *reply = [[NSJSONSerialization dataWithJSONObject:response options:0 error:nil] mutableCopy];
        [reply appendBytes:"\n" length:1];
        size_t sent = 0;
        while (sent<reply.length) {
            ssize_t count = write(fd,(const char *)reply.bytes+sent,reply.length-sent);
            if (count<=0) break;
            sent+=(size_t)count;
        }
        return;
    }
}

static void *server(void *argument) {
    int listener = (int)(intptr_t)argument;
    for (;;) { @autoreleasepool {
        fd_set fds; FD_ZERO(&fds); FD_SET(listener,&fds);
        struct timeval timeout = {.tv_sec=0,.tv_usec=100000};
        if (select(listener+1,&fds,NULL,NULL,&timeout)>0) {
            int fd = accept(listener,NULL,NULL);
            if (fd>=0) { serve(fd); close(fd); }
        }
        if (controlledCount() && NSProcessInfo.processInfo.systemUptime-lastUpdate>2) restoreAll();
    } }
    return NULL;
}

// An idle payload in this same Dock can hand its socket to a new build. Do
// not evict a controller or remove a socket owned by another process/user.
static bool replaceIdleSocket(struct sockaddr_un *addr) {
    struct stat before,after;
    if(lstat(addr->sun_path,&before))return errno==ENOENT;
    if(!S_ISSOCK(before.st_mode)||before.st_uid!=getuid())return false;
    int fd=socket(AF_UNIX,SOCK_STREAM,0);
    if(fd<0)return false;
    struct timeval timeout={.tv_sec=0,.tv_usec=200000};
    setsockopt(fd,SOL_SOCKET,SO_RCVTIMEO,&timeout,sizeof(timeout));
    setsockopt(fd,SOL_SOCKET,SO_SNDTIMEO,&timeout,sizeof(timeout));
    int noSigpipe=1;setsockopt(fd,SOL_SOCKET,SO_NOSIGPIPE,&noSigpipe,sizeof(noSigpipe));
    uid_t uid=0;gid_t gid=0;
    bool ok=false;
    int connected=connect(fd,(struct sockaddr *)addr,sizeof(*addr));
    // Dock can die without unlinking its socket. A refused, same-user socket
    // has no listening owner; retain the inode check before removing it.
    if(connected&&errno==ECONNREFUSED)ok=true;
    if(!connected&&!getpeereid(fd,&uid,&gid)&&uid==getuid()) {
        const char *hello="{\"op\":\"hello\"}\n";
        if(write(fd,hello,strlen(hello))==(ssize_t)strlen(hello)) {
            char bytes[4096];size_t used=0;
            while(used<sizeof(bytes)) {
                ssize_t n=read(fd,bytes+used,sizeof(bytes)-used);
                if(n<=0)break;
                used+=(size_t)n;
                char *nl=memchr(bytes,'\n',used);
                if(!nl)continue;
                id reply=[NSJSONSerialization JSONObjectWithData:[NSData dataWithBytes:bytes length:(NSUInteger)(nl-bytes)] options:0 error:nil];
                if([reply isKindOfClass:NSDictionary.class]) {
                    id pid=reply[@"pid"],owner=reply[@"uid"],controlled=reply[@"controlled"],version=reply[@"version"];
                    ok=[reply[@"ok"] isEqual:@YES]&&[pid isKindOfClass:NSNumber.class]&&[pid intValue]==getpid()&&
                        [owner isKindOfClass:NSNumber.class]&&[owner unsignedIntValue]==getuid()&&
                        [controlled isKindOfClass:NSNumber.class]&&[controlled unsignedIntValue]==0&&
                        [version isKindOfClass:NSNumber.class]&&[version intValue]>=1&&[version intValue]<=2;
                }
                break;
            }
        }
    }
    close(fd);
    return ok&&!lstat(addr->sun_path,&after)&&before.st_dev==after.st_dev&&before.st_ino==after.st_ino&&!unlink(addr->sun_path);
}

__attribute__((constructor)) static void load(void) { @autoreleasepool {
    // The payload must only be loaded into Dock; no arbitrary host mode.
    if (![NSRunningApplication.currentApplication.bundleIdentifier isEqual:@"com.apple.dock"] || !loadSkyLight(&sky)) return;
    void *h=dlopen("/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight",RTLD_NOW);
    getOwner=dlsym(h,"SLSGetWindowOwner");connectionPID=dlsym(h,"SLSConnectionGetPID");
    if(!getOwner||!connectionPID)return;
    Dl_info image;
    if(dladdr((void *)&load,&image)&&image.dli_fname)buildName=[[NSString stringWithUTF8String:image.dli_fname] lastPathComponent];
    char directory[80]; snprintf(directory,sizeof(directory),"/tmp/ribbonwm-%u",getuid());
    if (mkdir(directory,0700) && errno!=EEXIST) return;
    struct stat st;
    if (lstat(directory,&st) || !S_ISDIR(st.st_mode) || st.st_uid!=getuid() || (st.st_mode&0777)!=0700) return;
    struct sockaddr_un addr = {.sun_family=AF_UNIX};
    snprintf(addr.sun_path,sizeof(addr.sun_path),"%s/backend.sock",directory);
    int fd = socket(AF_UNIX,SOCK_STREAM,0);
    if (fd<0) return;
    if(!replaceIdleSocket(&addr)){close(fd);return;}
    if (bind(fd,(struct sockaddr *)&addr,sizeof(addr)) || chmod(addr.sun_path,0600) || listen(fd,8)) { close(fd); return; }
    saved = [NSMutableDictionary dictionary];stickySaved=[NSMutableDictionary dictionary];hasStickyAPI=loadStickyAPI(&stickyAPI);
    pthread_t thread;
    if (pthread_create(&thread,NULL,server,(void *)(intptr_t)fd)) { close(fd); unlink(addr.sun_path); return; }
    pthread_detach(thread);
    NSLog(@"[RibbonWM] ready at %s (state resets after 2s without apply)",addr.sun_path);
} }
