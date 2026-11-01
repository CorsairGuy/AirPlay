// Transparent ALSA observer for selected playback clients. No PCM writes or configuration changes.
#define _GNU_SOURCE
#include <stdint.h>
#include <stddef.h>
#include <sys/shm.h>
#include "source_ring.h"
extern void*dlsym(void*,const char*);extern char*getenv(const char*);extern long strtol(const char*,char**,int);extern int getpid(void);
#define NEXT ((void*)-1L)
typedef struct _snd_pcm snd_pcm_t;
typedef struct{void*addr;unsigned first,step;}Area;
struct Entry{snd_pcm_t*pcm;unsigned channels,rate;int format;const Area*areas;};
static struct Entry entries[8];static struct SourceRing*ring;static int disabled;
static __thread int inside;
static struct Entry*entry(snd_pcm_t*pcm){
 for(unsigned i=0;i<8;i++)if(entries[i].pcm==pcm)return &entries[i];
 unsigned slot;for(slot=0;slot<8;slot++)if(!entries[slot].pcm)break;if(slot==8)return NULL;
 int(*stream)(snd_pcm_t*)=dlsym(NEXT,"snd_pcm_stream");if(!stream||stream(pcm)!=0)return NULL;
 int(*alloc)(void**)=dlsym(NEXT,"snd_pcm_hw_params_malloc");void(*free_params)(void*)=dlsym(NEXT,"snd_pcm_hw_params_free");
 int(*current)(snd_pcm_t*,void*)=dlsym(NEXT,"snd_pcm_hw_params_current");int(*format)(const void*,int*)=dlsym(NEXT,"snd_pcm_hw_params_get_format");
 int(*rate)(const void*,unsigned*,int*)=dlsym(NEXT,"snd_pcm_hw_params_get_rate");int(*channels)(const void*,unsigned*)=dlsym(NEXT,"snd_pcm_hw_params_get_channels");
 if(!alloc||!free_params||!current||!format||!rate||!channels)return NULL;
 void*p=NULL;if(alloc(&p)<0)return NULL;struct Entry e={.pcm=pcm};int direction=0;
 int good=current(pcm,p)>=0&&format(p,&e.format)>=0&&rate(p,&e.rate,&direction)>=0&&channels(p,&e.channels)>=0;free_params(p);
 if(!good||e.channels!=2||(e.rate!=44100&&e.rate!=48000)||(e.format!=2&&e.format!=10))return NULL;
 entries[slot]=e;return &entries[slot];
}
static int attach(const struct Entry*e){
 if(disabled)return 0;
 if(!ring){const char*v=getenv("MINI2_AUDIO_TAP_KEY");if(!v){disabled=1;return 0;}int key=(int)strtol(v,NULL,0);if(key!=TAP_AIRPLAY_KEY&&key!=TAP_BLUETOOTH_KEY){disabled=1;return 0;}
 int id=shmget(key,sizeof(struct SourceRing),IPC_CREAT|0600);if(id<0)return 0;void*p=shmat(id,NULL,0);if(p==(void*)-1)return 0;ring=p;
 ring->magic=0;ring->version=1;ring->pid=(uint32_t)getpid();ring->rate=e->rate;ring->channels=2;ring->written=0;__sync_synchronize();ring->magic=TAP_MAGIC;
 }
 return ring->rate==e->rate;
}
static int16_t sample(const unsigned char*p,int format){if(format==2)return(int16_t)((unsigned)p[0]|(unsigned)p[1]<<8);return(int16_t)((unsigned)p[2]|(unsigned)p[3]<<8);}
static void mirror(struct Entry*e,const void*buffer,unsigned long offset,unsigned long frames,int mmap){
 if(!e||!attach(e)||frames>TAP_FRAMES)return;unsigned n=ring->written;unsigned size=e->format==2?2:4;
 for(unsigned long f=0;f<frames;f++){unsigned dst=((n+(unsigned)f)&(TAP_FRAMES-1))*2;for(unsigned c=0;c<2;c++){
 const unsigned char*p=mmap?(const unsigned char*)e->areas[c].addr+e->areas[c].first/8+(offset+f)*e->areas[c].step/8:(const unsigned char*)buffer+(f*2+c)*size;
 ring->samples[dst+c]=sample(p,e->format);
 }}__sync_synchronize();ring->written=n+(uint32_t)frames;
}
long snd_pcm_writei(snd_pcm_t*pcm,const void*buffer,unsigned long frames){
 static long(*real)(snd_pcm_t*,const void*,unsigned long);if(!real)real=dlsym(NEXT,"snd_pcm_writei");if(!real)return -38;
 if(inside)return real(pcm,buffer,frames);inside=1;long n=real(pcm,buffer,frames);if(n>0)mirror(entry(pcm),buffer,0,(unsigned long)n,0);inside=0;return n;
}
long snd_pcm_mmap_writei(snd_pcm_t*pcm,const void*buffer,unsigned long frames){
 static long(*real)(snd_pcm_t*,const void*,unsigned long);if(!real)real=dlsym(NEXT,"snd_pcm_mmap_writei");if(!real)return -38;
 if(inside)return real(pcm,buffer,frames);inside=1;long n=real(pcm,buffer,frames);if(n>0)mirror(entry(pcm),buffer,0,(unsigned long)n,0);inside=0;return n;
}
int snd_pcm_mmap_begin(snd_pcm_t*pcm,const Area**areas,unsigned long*offset,unsigned long*frames){
 static int(*real)(snd_pcm_t*,const Area**,unsigned long*,unsigned long*);if(!real)real=dlsym(NEXT,"snd_pcm_mmap_begin");if(!real)return -38;
 int n=real(pcm,areas,offset,frames);if(n>=0&&!inside){inside=1;struct Entry*e=entry(pcm);if(e)e->areas=*areas;inside=0;}return n;
}
long snd_pcm_mmap_commit(snd_pcm_t*pcm,unsigned long offset,unsigned long frames){
 static long(*real)(snd_pcm_t*,unsigned long,unsigned long);if(!real)real=dlsym(NEXT,"snd_pcm_mmap_commit");if(!real)return -38;
 if(inside)return real(pcm,offset,frames);inside=1;long n=real(pcm,offset,frames);if(n>0){struct Entry*e=entry(pcm);if(e&&e->areas)mirror(e,NULL,offset,(unsigned long)n,1);}inside=0;return n;
}
int snd_pcm_close(snd_pcm_t*pcm){static int(*real)(snd_pcm_t*);if(!real)real=dlsym(NEXT,"snd_pcm_close");for(unsigned i=0;i<8;i++)if(entries[i].pcm==pcm)entries[i].pcm=NULL;return real?real(pcm):-38;}
