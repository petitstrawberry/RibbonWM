// Exercise the actual inventory rectangle encoder, including WindowServer's
// successful-but-null rectangle sentinel. No AX access or user windows.
#import "query.m"
#include <assert.h>
int main(void) { @autoreleasepool {
    assert(!rectJSON(CGRectNull));
    assert(!rectJSON(CGRectInfinite));
    assert(!rectJSON(CGRectMake(NAN,0,800,600)));
    assert(!rectJSON(CGRectMake(0,INFINITY,800,600)));
    assert(!rectJSON(CGRectMake(0,0,INFINITY,600)));
    assert(!rectJSON(CGRectMake(0,0,800,-1)));
    assert(!usableBounds(CGRectZero));
    CGRect valid=CGRectMake(-1600,-900,800,600);
    assert(usableBounds(valid));
    NSArray *rows=@[@{@"bounds":rectJSON(valid),@"surface_bounds":rectJSON(CGRectNull)?:NSNull.null},
                    @{@"bounds":rectJSON(CGRectZero)}];
    assert([NSJSONSerialization isValidJSONObject:rows]);
    NSData *json=[NSJSONSerialization dataWithJSONObject:rows options:0 error:nil];
    assert(json.length>0);
    puts("PASS: null/infinite rectangles cannot reach inventory JSON; finite negative display coordinates and empty bounds serialize");
    return 0;
} }
