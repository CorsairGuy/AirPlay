#define _GNU_SOURCE
#include <stdio.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <signal.h>
#include <sys/shm.h>
#include <sys/prctl.h>
#include <time.h>
#include <unistd.h>
#include <errno.h>
static volatile sig_atomic_t stop;
static void done(int sig){(void)sig;stop=1;}
static int compatible(void){
 FILE *f=fopen("/proc/asound/card0/pcm0p/sub0/hw_params","r");if(!f)return 0;
 char s[256];int format=0,channels=0,rate=0,buffer=0;
 while(fgets(s,sizeof s,f)){if(!strcmp(s,"format: S16_LE\n"))format=1;
  if(sscanf(s,"channels: %d",&channels)==1)continue;
  if(sscanf(s,"rate: %d",&rate)==1)continue;
  if(sscanf(s,"buffer_size: %d",&buffer)==1)continue;}
 fclose(f);return format&&channels==2&&rate==48000&&buffer==7680;
}
static int64_t pointer(void){FILE*f=fopen("/proc/asound/card0/pcm0p/sub0/status","r");if(!f)return -1;char s[256];int64_t p=-1;while(fgets(s,sizeof s,f))if(sscanf(s,"hw_ptr : %ld",&p)==1)break;fclose(f);return p;}
static int output(const volatile int32_t *sum,size_t frames,int64_t first,size_t count,double gain){
 static int16_t previous[2];static uint64_t input_frames,output_frames;static unsigned char bytes[4096];static size_t used;
 for(size_t j=0;j<count;j++){
  int16_t next[2];size_t idx=(size_t)((first+(int64_t)j)%(int64_t)frames)*2;
  for(int c=0;c<2;c++){double v=sum[idx+c]*gain;if(v>8192)v=8192;if(v< -8192)v=-8192;next[c]=(int16_t)v;}
  if(input_frames){while(output_frames*48000<=input_frames*44100){
   double t=(double)(output_frames*48000-(input_frames-1)*44100)/44100.;
   if(t>=0&&t<=1){for(int c=0;c<2;c++){int16_t v=(int16_t)(previous[c]+(next[c]-previous[c])*t);bytes[used++]=(unsigned char)((uint16_t)v>>8);bytes[used++]=(unsigned char)v;}if(used==sizeof bytes){if(fwrite(bytes,1,used,stdout)!=used)return -1;used=0;}}
   output_frames++;
  }}
  memcpy(previous,next,sizeof previous);input_frames++;
 }
 if(used){if(fwrite(bytes,1,used,stdout)!=used)return -1;used=0;}return fflush(stdout);
}
int main(int argc,char **argv){
 pid_t parent=getppid();if(prctl(PR_SET_PDEATHSIG,SIGTERM)||getppid()!=parent||parent==1)return 1;
 int seconds=argc>1?atoi(argv[1]):60;double gain=argc>2?atof(argv[2]):1;if(seconds<0||seconds>600||gain<.01||gain>256)return 2;
 struct sigaction sa={0};sa.sa_handler=done;sigaction(SIGTERM,&sa,0);sigaction(SIGINT,&sa,0);signal(SIGPIPE,SIG_IGN);
 if(!compatible()){fprintf(stderr,"unsupported playback format; leaving audio untouched\n");return 1;}
 int id=shmget(1025,1,0400);if(id<0){perror("dmix sum buffer");return 1;}struct shmid_ds st;if(shmctl(id,IPC_STAT,&st))return 1;
 if(st.shm_segsz!=7680*2*4){fprintf(stderr,"unexpected dmix buffer size\n");return 1;}
 const volatile int32_t *sum=shmat(id,0,SHM_RDONLY);if(sum==(void*)-1)return 1;
 int64_t last=pointer();if(last<960)return 1;last-=960;struct timespec start,now;clock_gettime(CLOCK_MONOTONIC,&start);int dropped=0;long total=0;
 fprintf(stderr,"passive dmix tap: read-only, 48000 -> 44100 stereo, gain %.1f, peak ceiling 0.25\n",gain);
 while(!stop){clock_gettime(CLOCK_MONOTONIC,&now);if(seconds&&now.tv_sec-start.tv_sec>=seconds)break;int64_t end=pointer()-960;
  if(shmget(1025,1,0400)!=id){fprintf(stderr,"dmix segment replaced; stopping\n");break;}
  if(end<0){usleep(5000);continue;}
  if(end<last){fprintf(stderr,"playback pointer reset; resynchronizing read position\n");last=end;dropped++;}
  if(end-last>4800){last=end;dropped++;}
  if(end>last){size_t n=(size_t)(end-last);if(output(sum,7680,last,n,gain))break;total+=(long)n;last=end;}usleep(5000);
 }
 shmdt((void*)sum);fprintf(stderr,"passive tap stopped: %ld frames, %d overruns\n",total,dropped);return 0;
}
