#define _GNU_SOURCE
#include <stdio.h>
#include <stdint.h>
#include <stdlib.h>
#include <signal.h>
#include <sys/shm.h>
#include <sys/prctl.h>
#include <time.h>
#include <unistd.h>
#include <errno.h>
#include "source_ring.h"
static volatile sig_atomic_t running=1;
static void stop(int n){(void)n;running=0;}
static int allowed_pid(uint32_t pid){return pid>1&&kill((pid_t)pid,0)==0;}
int main(int argc,char**argv){
 if(argc!=3)return 2;int key=atoi(argv[1]);if(key!=TAP_AIRPLAY_KEY&&key!=TAP_BLUETOOTH_KEY)return 2;
 pid_t parent=getppid();if(prctl(PR_SET_PDEATHSIG,SIGTERM)||parent==1||getppid()!=parent)return 1;
 signal(SIGTERM,stop);signal(SIGINT,stop);signal(SIGPIPE,SIG_IGN);
 int id=shmget(key,sizeof(struct SourceRing),0400);if(id<0)return 1;const volatile struct SourceRing*r=shmat(id,NULL,SHM_RDONLY);if(r==(void*)-1)return 1;
 if(r->magic!=TAP_MAGIC||r->version!=1||r->channels!=2||(r->rate!=44100&&r->rate!=48000)||!allowed_pid(r->pid))return 1;
 uint32_t pid=r->pid,rate=r->rate,last=r->written;uint64_t input_frames=0,output_frames=0;int16_t previous[2]={0};unsigned char bytes[8192];size_t used=0;double gain=.25;
 fprintf(stderr,"source-only reader: %u Hz stereo, no local output or speech mixer\n",rate);
 while(running){if(!allowed_pid(pid)||r->pid!=pid||r->rate!=rate||r->magic!=TAP_MAGIC)break;
  uint32_t end=r->written;__sync_synchronize();uint32_t count=end-last;if(count>TAP_FRAMES/2){last=end;continue;}
  int duck=0;FILE*f=fopen(argv[2],"r");if(f){duck=fgetc(f)=='1';fclose(f);}double target=duck?.10:.25;
  for(uint32_t j=0;j<count&&running;j++){
   size_t at=((last+j)&(TAP_FRAMES-1))*2;int16_t next[2]={r->samples[at],r->samples[at+1]};
   if(input_frames){while(output_frames*rate<=input_frames*44100){double t=(double)(output_frames*rate-(input_frames-1)*44100)/44100.;if(t>=0&&t<=1){gain+=(target-gain)/2205.;for(int c=0;c<2;c++){int v=(int)((previous[c]+(next[c]-previous[c])*t)*gain);if(v>8192)v=8192;if(v< -8192)v=-8192;uint16_t value=(uint16_t)(int16_t)v;bytes[used++]=(unsigned char)(value>>8);bytes[used++]=(unsigned char)value;}if(used==sizeof bytes){if(fwrite(bytes,1,used,stdout)!=used){running=0;break;}used=0;}}output_frames++;}}
   previous[0]=next[0];previous[1]=next[1];input_frames++;
  }
  last=end;if(used){if(fwrite(bytes,1,used,stdout)!=used)break;used=0;}if(fflush(stdout))break;usleep(5000);
 }
 shmdt((void*)r);return 0;
}
