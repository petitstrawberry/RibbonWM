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
