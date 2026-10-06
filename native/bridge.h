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
int ribbon_ax_request_permission(void);
int ribbon_resize_window(uint32_t wid,int expected_pid,RibbonRect rect);
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
