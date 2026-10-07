// Exercise the actual inventory rectangle encoder, including WindowServer's
// successful-but-null rectangle sentinel. No AX access or user windows.
#import "query.m"
#include <assert.h>
#include <pthread.h>
static NSArray *decode(char *raw) {
    assert(raw);NSData *data=[[NSString stringWithUTF8String:raw] dataUsingEncoding:NSUTF8StringEncoding];
    NSArray *rows=[NSJSONSerialization JSONObjectWithData:data options:0 error:nil];ribbon_free(raw);assert(rows);return rows;
}
typedef struct { pthread_t caller; unsigned count; BOOL fail; } ProgressCheck;
static int checkProgress(void *context) {
    ProgressCheck *check=context;assert(pthread_equal(pthread_self(),check->caller));
    check->count++;return check->fail;
}
int main(int argc,char **argv) { @autoreleasepool {
    ProgressCheck progress={.caller=pthread_self()};
    __block BOOL completed=NO;
    assert(geometryRequest(^{ usleep(30000);completed=YES;return (AXError)kAXErrorSuccess; },checkProgress,&progress)==0);
    assert(completed&&progress.count>1);
    progress.count=0;progress.fail=YES;completed=NO;
    assert(geometryRequest(^{ usleep(30000);completed=YES;return (AXError)kAXErrorSuccess; },checkProgress,&progress)==kAXErrorFailure);
    assert(completed&&progress.count==1);
    progress.count=0;progress.fail=NO;
    assert(geometryRequest(^{ return (AXError)kAXErrorInvalidUIElement; },checkProgress,&progress)==kAXErrorInvalidUIElement);
    puts("PASS: blocked AX requests keep caller-thread presentation live, cancellation joins the request, and AX errors propagate");
    // Stable geometry must be observed after the last owner/size change; a
    // timeout or invalid intervening sample cannot be counted as settlement.
    RibbonSettlement settlement={.deadline=2};
    CGAffineTransform a=CGAffineTransformIdentity,b=CGAffineTransformMakeTranslation(20,30);
    assert(sampleSettlement(&settlement,YES,a,1)==0);
    assert(sampleSettlement(&settlement,YES,a,1.01)==0);
    assert(sampleSettlement(&settlement,YES,b,1.04)==0);
    assert(sampleSettlement(&settlement,YES,b,1.05)==0);
    assert(sampleSettlement(&settlement,NO,b,1.09)==0);
    assert(sampleSettlement(&settlement,YES,b,1.10)==0);
    assert(sampleSettlement(&settlement,YES,b,1.11)==0);
    assert(sampleSettlement(&settlement,YES,b,1.14)==0);
    assert(sampleSettlement(&settlement,YES,b,1.17)==1);
    assert(sampleSettlement(&settlement,YES,b,2)==kAXErrorCannotComplete);
    assert(!activeSession(nil));
    assert(!activeSession(@{(__bridge NSString *)kCGSessionOnConsoleKey:@NO}));
    assert(!activeSession(@{(__bridge NSString *)kCGSessionOnConsoleKey:@YES,@"CGSSessionScreenIsLocked":@YES}));
    assert(activeSession(@{(__bridge NSString *)kCGSessionOnConsoleKey:@YES}));
    assert(activeSession(@{(__bridge NSString *)kCGSessionOnConsoleKey:@YES,@"CGSSessionScreenIsLocked":@NO}));
    if(argc==2&&!strcmp(argv[1],"--inventory-profile")) {
        for(int iteration=0;iteration<5;iteration++) {
            double begin=NSProcessInfo.processInfo.systemUptime;
            NSArray *full=decode(ribbon_query_json(1));double fullEnd=NSProcessInfo.processInfo.systemUptime;
            NSArray *summary=decode(ribbon_query_json(3));double summaryEnd=NSProcessInfo.processInfo.systemUptime;
            uint32_t *ids=calloc(summary.count,sizeof(uint32_t));size_t count=0;
            for(NSDictionary *row in summary) {
                assert(row[@"surface_bounds"]==NSNull.null&&![row[@"sticky_known"] boolValue]);
                if([row[@"layer"] intValue]==0&&[row[@"onscreen"] boolValue]&&
                    [row[@"bounds"][@"width"] doubleValue]>=100&&[row[@"bounds"][@"height"] doubleValue]>=100)
                    ids[count++]=[row[@"id"] unsignedIntValue];
            }
            NSArray *details=decode(ribbon_query_windows_json(ids,count));
            for(NSDictionary *row in details) {
                BOOL requested=NO;for(size_t i=0;i<count;i++)if(ids[i]==[row[@"id"] unsignedIntValue])requested=YES;
                assert(requested);
            }
            double end=NSProcessInfo.processInfo.systemUptime;free(ids);
            printf("full=%lu selected=%lu full_ms=%.2f summary_ms=%.2f detail_ms=%.2f\n",
                (unsigned long)full.count,(unsigned long)details.count,(fullEnd-begin)*1000,(summaryEnd-fullEnd)*1000,(end-summaryEnd)*1000);
        }
        return 0;
    }
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
    puts("PASS: settlement and session guards; null/infinite rectangles cannot reach inventory JSON; finite negative display coordinates and empty bounds serialize");
    return 0;
} }
