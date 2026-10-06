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

@interface RibbonSavedWindowV3 : NSObject
@property uint32_t wid;
@property CGRect bounds;
@property CGAffineTransform transform;
@property CFTypeRef clip;
@property pid_t pid;
@end
@implementation RibbonSavedWindowV3
@end

static SkyLight sky;
static NSMutableDictionary<NSNumber *, RibbonSavedWindowV3 *> *saved;
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

static unsigned restoreWindows(NSSet<NSNumber *> *retained) {
    unsigned failed=0;
    for (RibbonSavedWindowV3 *w in saved.allValues) {
        if ([retained containsObject:@(w.wid)]) continue;
        if (ownerPID(w.wid)==w.pid) {
            CGError et=sky.setTransform(sky.connection(),w.wid,w.transform);
            CGError ec=sky.setClip(sky.connection(),w.wid,w.clip);
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
    RibbonSavedWindowV3 *w=saved[@(wid)];
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
        [ids addObject:@((uint32_t)v)];
    }
    NSMutableSet *allIDs=[ids mutableCopy];[allIDs unionSet:stickyIDs];
    if(allIDs.count>128)return error(@"At most 128 total windows");
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
        RibbonSavedWindowV3 *w=saved[@(wid)];pid_t pid=ownerPID(wid);
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
            if(eb||et||ec||!clip) {
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
            w=[RibbonSavedWindowV3 new];w.wid=wid;w.bounds=b;w.transform=t;w.clip=clip;w.pid=pid;saved[@(wid)]=w;
        }
        CGRect local=CGRectZero;
        if(u[@"clip"]!=NSNull.null) {rect(u[@"clip"],&c);local=CGRectMake(c.origin.x-f.origin.x,c.origin.y-f.origin.y,c.size.width,c.size.height);}
        CFTypeRef region=NULL;CGError er=sky.newRegion(&local,&region);
        CGError ec=er?er:sky.setClip(sky.connection(),wid,region);
        CGError et=ec?ec:sky.setTransform(sky.connection(),wid,CGAffineTransformMakeTranslation(-f.origin.x,-f.origin.y));
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
static NSDictionary *handle(id r) {
    if(![r isKindOfClass:NSDictionary.class])return error(@"Expected JSON object");
    if([r[@"op"] isEqual:@"hello"])return @{@"ok":@YES,@"version":@2,@"capabilities":hasStickyAPI?@[@"sticky"]:@[],@"build":buildName?:@"",@"pid":@(getpid()),@"uid":@(getuid()),@"controlled":@(controlledCount())};
    NSString *session=r[@"session"];
    if(![session isKindOfClass:NSString.class]||session.length==0||session.length>128)return error(@"Invalid session");
    if(controller&&![controller isEqual:session])return error(@"Another controller holds the lease");
    if([r[@"op"] isEqual:@"reset"]) {unsigned failed=restoreAll();return failed?error(@"Restore failed; watchdog will retry"):@{@"ok":@YES};}
    if([r[@"op"] isEqual:@"frame"])return frame(r);
    if([r[@"op"] isEqual:@"sticky"])return setSticky(r[@"window"],session);
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
