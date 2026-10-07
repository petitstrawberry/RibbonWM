#pragma once
#import <Cocoa/Cocoa.h>
#include <dlfcn.h>

// Private ABI: validated symbols on macOS 26.6.2, arm64.
typedef struct {
    int (*connection)(void);
    CGError (*getTransform)(int, uint32_t, CGAffineTransform *);
    CGError (*setTransform)(int, uint32_t, CGAffineTransform);
    CGError (*copyClip)(int, uint32_t, CFTypeRef *);
    CGError (*setClip)(int, uint32_t, CFTypeRef);
    CGError (*newRegion)(const CGRect *, CFTypeRef *);
    CGError (*releaseRegion)(CFTypeRef);
    CGError (*getBounds)(int, uint32_t, CGRect *);
    CGError (*moveWithGroup)(int, uint32_t, CGPoint *);
    CGError (*disableUpdates)(int);
    CGError (*enableUpdates)(int);
} SkyLight;

static bool loadSkyLight(SkyLight *s) {
    void *h = dlopen("/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight", RTLD_NOW);
    if (!h) return false;
#define LOAD(field, symbol) do { s->field = dlsym(h, symbol); if (!s->field) return false; } while (0)
    LOAD(connection, "SLSMainConnectionID");
    LOAD(getTransform, "SLSGetWindowTransform");
    LOAD(setTransform, "SLSSetWindowTransform");
    LOAD(copyClip, "SLSCopyWindowClipShape");
    LOAD(setClip, "SLSSetWindowClipShape");
    LOAD(newRegion, "CGSNewRegionWithRect");
    LOAD(releaseRegion, "CGSReleaseRegion");
    LOAD(getBounds, "SLSGetWindowBounds");
    LOAD(moveWithGroup, "SLSMoveWindowWithGroup");
    LOAD(disableUpdates, "SLSDisableUpdate");
    LOAD(enableUpdates, "SLSReenableUpdate");
#undef LOAD
    return true;
}

// Translate in compositor coordinates, leaving AppKit's logical frame alone.
static inline CGAffineTransform translatedTransform(CGAffineTransform original, CGFloat dx, CGFloat dy) {
    return CGAffineTransformTranslate(original, -dx, -dy);
}

// Optional sticky ABI; failure must not disable ordinary clipping.
typedef struct {
    CFTypeRef (*query)(int, CFArrayRef, int);
    CFTypeRef (*iterator)(CFTypeRef);
    bool (*advance)(CFTypeRef);
    uint64_t (*tags)(CFTypeRef);
    CGError (*set)(int,uint32_t,uint64_t *,size_t);
    CGError (*clear)(int,uint32_t,uint64_t *,size_t);
} RibbonStickyAPI;
static inline bool loadStickyAPI(RibbonStickyAPI *s) {
    void *h=dlopen("/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight",RTLD_NOW);
    if(!h)return false;
#define STICKY_LOAD(field,symbol) do {s->field=dlsym(h,symbol);if(!s->field)return false;} while(0)
    STICKY_LOAD(query,"SLSWindowQueryWindows");
    STICKY_LOAD(iterator,"SLSWindowQueryResultCopyWindows");
    STICKY_LOAD(advance,"SLSWindowIteratorAdvance");
    STICKY_LOAD(tags,"SLSWindowIteratorGetTags");
    STICKY_LOAD(set,"SLSSetWindowTags");
    STICKY_LOAD(clear,"SLSClearWindowTags");
#undef STICKY_LOAD
    return true;
}
static inline bool readSticky(RibbonStickyAPI *s,int cid,uint32_t wid,bool *value) {
    CFTypeRef q=s->query(cid,(__bridge CFArrayRef)@[@(wid)],1);
    if(!q)return false;
    CFTypeRef i=s->iterator(q);bool ok=i&&s->advance(i);
    if(ok)*value=(s->tags(i)&(UINT64_C(1)<<11))!=0;
    if(i)CFRelease(i);CFRelease(q);return ok;
}

// Optional normal/floating window-level operations, independent of geometry.
typedef struct {
    CGError (*get)(int,uint32_t,int *);
    CGError (*set)(int,uint32_t,int);
} RibbonLevelAPI;
// The legacy GetWindowLevel symbol can return stale values on newer macOS.
// Use WindowServer's iterator, as with the existing sticky tag query.
static inline CGError readWindowLevel(int cid,uint32_t wid,int *level) {
    static CFTypeRef (*query)(int,CFArrayRef,int),(*iterator)(CFTypeRef);
    static bool (*advance)(CFTypeRef);
    static int (*getLevel)(CFTypeRef);
    static dispatch_once_t once;
    dispatch_once(&once,^{
        void *h=dlopen("/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight",RTLD_NOW);
        query=dlsym(h,"SLSWindowQueryWindows");iterator=dlsym(h,"SLSWindowQueryResultCopyWindows");
        advance=dlsym(h,"SLSWindowIteratorAdvance");getLevel=dlsym(h,"SLSWindowIteratorGetLevel");
    });
    if(!query||!iterator||!advance||!getLevel)return kCGErrorFailure;
    CFTypeRef q=query(cid,(__bridge CFArrayRef)@[@(wid)],1),i=q?iterator(q):NULL;
    bool ok=i&&advance(i);if(ok)*level=getLevel(i);
    if(i)CFRelease(i);if(q)CFRelease(q);return ok?kCGErrorSuccess:kCGErrorFailure;
}
static inline bool loadLevelAPI(RibbonLevelAPI *api) {
    void *h=dlopen("/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight",RTLD_NOW);
    if(!h)return false;
    api->get=readWindowLevel;api->set=dlsym(h,"SLSSetWindowLevel");
    return api->get&&api->set;
}
