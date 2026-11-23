/* Private MRV1 video IPC consumer. No AirPlay, pairing or network implementation. */
#include <gst/gst.h>
#include <gst/app/gstappsrc.h>
#include <stdio.h>
#include <stdint.h>
#include <string.h>
#include <fcntl.h>
#include <io.h>
#include <windows.h>

static uint32_t le32(const unsigned char *p) {
    return (uint32_t)p[0]|((uint32_t)p[1]<<8)|((uint32_t)p[2]<<16)|((uint32_t)p[3]<<24);
}
static uint64_t le64(const unsigned char *p) { return le32(p)|((uint64_t)le32(p+4)<<32); }
static GstElement *pipeline, *source, *sink;
static unsigned current_codec;
static guint64 first_pts, base_pts, frames;
static gboolean anchored;
static gint64 started_us;
static DWORD current_pid;
static BOOL CALLBACK foreground_window(HWND hwnd, LPARAM unused) {
    DWORD owner=0; (void)unused;
    GetWindowThreadProcessId(hwnd,&owner);
    if (owner!=current_pid || !IsWindowVisible(hwnd) || GetWindow(hwnd,GW_OWNER)) return TRUE;
    SetWindowPos(hwnd,HWND_TOPMOST,0,0,0,0,SWP_NOMOVE|SWP_NOSIZE|SWP_SHOWWINDOW);
    ShowWindow(hwnd,SW_SHOWMAXIMIZED);
    HWND foreground=GetForegroundWindow();
    DWORD current=GetCurrentThreadId();
    DWORD target=GetWindowThreadProcessId(hwnd,NULL);
    DWORD front=foreground?GetWindowThreadProcessId(foreground,NULL):0;
    BOOL linked_target=(target!=current) && AttachThreadInput(current,target,TRUE);
    BOOL linked_front=(front && front!=current && front!=target) && AttachThreadInput(current,front,TRUE);
    BringWindowToTop(hwnd);
    BOOL requested=SetForegroundWindow(hwnd);
    if(linked_front)AttachThreadInput(current,front,FALSE);
    if(linked_target)AttachThreadInput(current,target,FALSE);
    if(GetForegroundWindow()==hwnd) { fprintf(stderr,"window foreground acquired\n");return FALSE; }
    fprintf(stderr,"window foreground pending requested=%d error=%lu\n",requested,(unsigned long)GetLastError());
    return TRUE;
}
static DWORD WINAPI activate_window(LPVOID unused) {
    (void)unused;
    for (int i=0;i<50;i++) {
        if (!EnumWindows(foreground_window,0)) break;
        Sleep(100);
    }
    return 0;
}
static void element_added(GstBin *bin, GstBin *sub, GstElement *element, gpointer data) {
    (void)bin;(void)sub;(void)data;
    GstElementFactory *factory=gst_element_get_factory(element);
    if(factory) fprintf(stderr,"pipeline element=%s\n",gst_plugin_feature_get_name(GST_PLUGIN_FEATURE(factory)));
}

static void close_pipeline(void) {
    if (!pipeline) return;
    gst_element_set_state(pipeline,GST_STATE_NULL);
    if (source) gst_object_unref(source);
    if (sink) gst_object_unref(sink);
    gst_object_unref(pipeline);
    pipeline=source=sink=NULL; anchored=FALSE;
}
static int open_pipeline(unsigned codec) {
    const char *parser=codec==1?"h264parse":"h265parse";
    const char *media=codec==1?"video/x-h264":"video/x-h265";
    gchar *description=g_strdup_printf(
        "appsrc name=frames is-live=true format=time do-timestamp=false block=true max-bytes=16777216 ! "
        "queue max-size-buffers=8 max-size-bytes=0 max-size-time=0 ! %s ! decodebin ! "
        "d3d11videosink name=screen sync=true async=false qos=false max-lateness=-1 "
        "fullscreen-toggle-mode=property+alt-enter fullscreen=true force-aspect-ratio=true",parser);
    GError *error=NULL;
    pipeline=gst_parse_launch(description,&error);g_free(description);
    if (error) { fprintf(stderr,"pipeline: %s\n",error->message);g_error_free(error);close_pipeline();return 0; }
    source=gst_bin_get_by_name(GST_BIN(pipeline),"frames");
    sink=gst_bin_get_by_name(GST_BIN(pipeline),"screen");
    if (!source || !sink) { close_pipeline();return 0; }
    g_signal_connect(pipeline,"deep-element-added",G_CALLBACK(element_added),NULL);
    GstCaps *caps=gst_caps_new_simple(media,"stream-format",G_TYPE_STRING,"byte-stream","alignment",G_TYPE_STRING,"au",NULL);
    gst_app_src_set_caps(GST_APP_SRC(source),caps);gst_caps_unref(caps);
    if (gst_element_set_state(pipeline,GST_STATE_PLAYING)==GST_STATE_CHANGE_FAILURE) { close_pipeline();return 0; }
    current_codec=codec;
    current_pid=GetCurrentProcessId();
    HANDLE thread=CreateThread(NULL,0,activate_window,NULL,0,NULL);
    if(thread)CloseHandle(thread);
    return 1;
}
static void report(void) {
    GstStructure *stats=NULL;guint64 rendered=0,dropped=0;
    g_object_get(sink,"stats",&stats,NULL);
    if (stats) {
        gst_structure_get_uint64(stats,"rendered",&rendered);
        gst_structure_get_uint64(stats,"dropped",&dropped);
        gst_structure_free(stats);
    }
    fprintf(stderr,"viewer frames=%llu rendered=%llu dropped=%llu elapsed_ms=%lld\n",
        (unsigned long long)frames,(unsigned long long)rendered,(unsigned long long)dropped,
        (long long)((g_get_monotonic_time()-started_us)/1000));
    fflush(stderr);
}
int main(int argc,char **argv) {
    _setmode(_fileno(stdin),_O_BINARY);gst_init(&argc,&argv);started_us=g_get_monotonic_time();
    unsigned char header[20];int result=0;
    while (fread(header,1,sizeof header,stdin)==sizeof header) {
        unsigned kind=header[4],codec=header[5];uint32_t size=le32(header+8);uint64_t pts=le64(header+12);
        if (memcmp(header,"MRV1",4)||header[6]||header[7]||size>32u*1024u*1024u || (kind!=1&&kind!=2)) { result=2;break; }
        if (kind==1 && codec!=1 && codec!=2) { result=2;break; }
        GstBuffer *buffer=gst_buffer_new_allocate(NULL,size,NULL);GstMapInfo map;
        if (!buffer || !gst_buffer_map(buffer,&map,GST_MAP_WRITE)) { if(buffer)gst_buffer_unref(buffer);result=2;break; }
        size_t got=fread(map.data,1,size,stdin);gst_buffer_unmap(buffer,&map);
        if (got!=size) { gst_buffer_unref(buffer);result=2;break; }
        if (kind==1) {
            if (!pipeline || codec!=current_codec) { close_pipeline();if(!open_pipeline(codec)){gst_buffer_unref(buffer);result=3;break;} }
            GST_BUFFER_FLAG_SET(buffer,GST_BUFFER_FLAG_HEADER);
            GST_BUFFER_PTS(buffer)=GST_CLOCK_TIME_NONE;
        } else {
            if (!pipeline) { gst_buffer_unref(buffer);result=2;break; }
            if (!anchored) {
                GstClock *clock=gst_element_get_clock(pipeline);
                if (!clock) { gst_buffer_unref(buffer);result=3;break; }
                GstClockTime now=gst_clock_get_time(clock),base=gst_element_get_base_time(pipeline);
                gst_object_unref(clock);first_pts=pts;base_pts=(now>=base?now-base:0)+80*GST_MSECOND;anchored=TRUE;
            }
            /* Preserve sender spacing; do not timestamp OS pipe chunks. */
            if (pts<first_pts || pts-first_pts>GST_CLOCK_TIME_NONE-base_pts-1) { gst_buffer_unref(buffer);result=2;break; }
            GST_BUFFER_PTS(buffer)=base_pts+(pts-first_pts);frames++;
        }
        if (gst_app_src_push_buffer(GST_APP_SRC(source),buffer)!=GST_FLOW_OK) {result=3;break;}
        GstBus *bus=gst_element_get_bus(pipeline);GstMessage *error=gst_bus_pop_filtered(bus,GST_MESSAGE_ERROR);
        gst_object_unref(bus);
        if(error){GError *err=NULL;gchar *debug=NULL;gst_message_parse_error(error,&err,&debug);fprintf(stderr,"viewer: %s\n",err->message);g_error_free(err);g_free(debug);gst_message_unref(error);result=3;break;}
        if (kind==2 && frames%300==0) report();
    }
    if(pipeline) report();
    close_pipeline();
    return result;
}
