#pragma once
#include <stdint.h>
#include <stddef.h>
#include <math.h>
typedef struct { double x,y,width,height; } RibbonRect;
static inline RibbonRect ribbon_display_viewport(RibbonRect app,RibbonRect visible,RibbonRect cg,double reservedTop) {
    double top=fmax(app.y+app.height-visible.y-visible.height,reservedTop);
    double bottom=visible.y-app.y;
    return (RibbonRect){cg.x+visible.x-app.x,cg.y+top,visible.width,fmax(1,cg.height-top-bottom)};
}
static inline RibbonRect ribbon_outer_to_ax(RibbonRect target,RibbonRect ax,RibbonRect outer) {
    return (RibbonRect){target.x+ax.x-outer.x,target.y+ax.y-outer.y,
        target.width+ax.width-outer.width,target.height+ax.height-outer.height};
}
typedef struct { uint32_t wid; RibbonRect bounds; } RibbonDemoWindow;
typedef struct { RibbonRect viewport; RibbonDemoWindow windows[3]; } RibbonDemoInfo;
typedef struct { uint32_t wid,visible; double dx,dy; RibbonRect clip; } RibbonDemoFrame;
typedef struct {
    void *context;
    int (*initialize)(void *,const RibbonDemoInfo *);
    size_t (*update)(void *,double,RibbonDemoFrame *,size_t);
    void (*event)(void *,int,double);
} RibbonCallbacks;
int ribbon_demo_run(const RibbonCallbacks *,int test,double seconds,double left,double width);
char *ribbon_query_json(int kind);
void ribbon_free(void *);
int ribbon_ax_trusted(void);
void ribbon_wait_for_events(double seconds);
void ribbon_permission_host_initialize(void);
int ribbon_owned_probe_accessible(int pid);
int ribbon_ax_request_permission(void);
int ribbon_resize_window(uint32_t wid,int expected_pid,RibbonRect rect);
typedef int (*RibbonGeometryProgress)(void *);
int ribbon_resize_window_observed(uint32_t wid,int expected_pid,RibbonRect rect,RibbonGeometryProgress progress,void *context);
int ribbon_window_geometry(uint32_t wid,int expected_pid,RibbonRect *rect);
int ribbon_restore_window(uint32_t wid,int expected_pid,RibbonRect rect);
int ribbon_focus_window(uint32_t wid,int expected_pid);
int ribbon_frontmost_pid(void);
uint32_t ribbon_focused_window(int expected_pid);
int ribbon_window_manageable(uint32_t wid,int expected_pid);
int ribbon_window_owner(uint32_t wid);
void ribbon_forget_window(uint32_t wid);
void ribbon_pointer(double *x,double *y);
int ribbon_left_mouse_down(void);
typedef struct { uint64_t identity; double x,y; uint32_t phase; } RibbonTouch;
typedef struct {
    double dx,dy,x,y,time;
    uint64_t flags;
    uint32_t phase,momentum,precise,kind,count;
    RibbonTouch touches[8];
} RibbonScrollEvent;
typedef int (*RibbonScrollCallback)(void *,const RibbonScrollEvent *);
int ribbon_start_scroll(RibbonScrollCallback,void *);
void ribbon_stop_scroll(void);
int ribbon_scroll_alive(void);
double ribbon_input_time(void);
int ribbon_input_trusted(void);
int ribbon_request_input_permission(void);
void ribbon_probe_show(const char *text);
uint32_t ribbon_probe_action(void);
void ribbon_probe_close(void);
enum { RIBBON_EVENT_WINDOWS=1,RIBBON_EVENT_FOCUS=2,RIBBON_EVENT_GEOMETRY=4,RIBBON_EVENT_APPS=8 };
int ribbon_watch_application(int pid);
void ribbon_unwatch_application(int pid);
void ribbon_watch_ax_element(const void *element,int pid);
uint32_t ribbon_events(void);
typedef struct { uint32_t wid; int32_t pid; } RibbonClosedWindow;
uint32_t ribbon_ax_window_id(const void *element);
size_t ribbon_take_closed_windows(RibbonClosedWindow *windows,size_t capacity,int refresh);
void ribbon_stop_observing(void);
