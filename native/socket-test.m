// Exercise the exact payload socket handoff without loading anything into Dock.
#import "payload.m"

static void check(bool value,const char *message) {
    if(!value){fprintf(stderr,"FAIL: %s\n",message);exit(1);}
    printf("PASS: %s\n",message);
}
int main(void) { @autoreleasepool {
    char directory[]="/tmp/ribbon-socket-XXXXXX";
    check(mkdtemp(directory)!=NULL,"private test directory");
    struct sockaddr_un addr={.sun_family=AF_UNIX};
    snprintf(addr.sun_path,sizeof(addr.sun_path),"%s/backend.sock",directory);
    check(replaceIdleSocket(&addr),"missing socket permits startup");
    int fd=socket(AF_UNIX,SOCK_STREAM,0);
    check(fd>=0&&!bind(fd,(struct sockaddr *)&addr,sizeof(addr)),"create stale socket");
    close(fd);
    check(replaceIdleSocket(&addr)&&access(addr.sun_path,F_OK)!=0,"stale socket after Dock death permits startup");
    for(int busy=0;busy<2;busy++) {
        fd=socket(AF_UNIX,SOCK_STREAM,0);
        check(fd>=0&&!bind(fd,(struct sockaddr *)&addr,sizeof(addr))&&!listen(fd,1),"create live test listener");
        int listener=fd;
        dispatch_group_t group=dispatch_group_create();
        dispatch_group_async(group,dispatch_get_global_queue(QOS_CLASS_USER_INITIATED,0),^{
            int client=accept(listener,NULL,NULL);char request[256];
            if(client>=0) {
                read(client,request,sizeof(request));
                NSData *json=[NSJSONSerialization dataWithJSONObject:@{@"ok":@YES,@"pid":@(getpid()),@"uid":@(getuid()),@"version":@2,@"controlled":@(busy)} options:0 error:nil];
                write(client,json.bytes,json.length);write(client,"\n",1);close(client);
            }
        });
        bool replaced=replaceIdleSocket(&addr);
        dispatch_group_wait(group,DISPATCH_TIME_FOREVER);close(listener);
        check(busy?!replaced&&access(addr.sun_path,F_OK)==0:replaced&&access(addr.sun_path,F_OK)!=0,
            busy?"active controller is retained":"idle backend in same process hands over");
        if(busy)unlink(addr.sun_path);
    }
    FILE *file=fopen(addr.sun_path,"w");check(file!=NULL,"create non-socket path");fclose(file);
    check(!replaceIdleSocket(&addr)&&access(addr.sun_path,F_OK)==0,"ordinary file is never removed");
    unlink(addr.sun_path);rmdir(directory);return 0;
} }
