// Standalone content reader. It never opens an ALSA device.
#include <libavformat/avformat.h>
#include <libavcodec/avcodec.h>
#include <libavutil/aes_ctr.h>
#include <libavutil/channel_layout.h>
#include <libswresample/swresample.h>
#include <stdio.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <signal.h>
#include <sys/prctl.h>
#include <unistd.h>
static volatile sig_atomic_t running=1;
static void stop(int n){(void)n;running=0;}
struct Memory{uint8_t *data;int64_t size,at;};
static int read_data(void*opaque,uint8_t*b,int size){struct Memory*m=opaque;if(m->at>=m->size)return AVERROR_EOF;int64_t left=m->size-m->at;if(size>left)size=(int)left;memcpy(b,m->data+m->at,size);m->at+=size;return size;}
static int64_t seek_data(void*opaque,int64_t offset,int whence){struct Memory*m=opaque;if(whence==AVSEEK_SIZE)return m->size;int64_t pos=(whence==SEEK_SET?0:whence==SEEK_CUR?m->at:whence==SEEK_END?m->size:-1)+offset;if(pos<0||pos>m->size)return -1;return m->at=pos;}
int main(int argc,char**argv){
 if(argc!=5){fprintf(stderr,"usage: content-reader ENCRYPTED_FILE KEY_FILE OFFSET_SECONDS DUCK_FILE\n");return 2;}
 pid_t parent=getppid();if(prctl(PR_SET_PDEATHSIG,SIGTERM)||getppid()!=parent)return 1;signal(SIGTERM,stop);signal(SIGINT,stop);signal(SIGPIPE,SIG_IGN);av_log_set_level(AV_LOG_ERROR);
 FILE*f=fopen(argv[1],"rb");if(!f)return 1;fseek(f,0,SEEK_END);long size=ftell(f);rewind(f);if(size<=0||size>16*1024*1024)return 1;
 struct Memory mem={.size=size};mem.data=av_malloc(size);if(!mem.data||fread(mem.data,1,size,f)!=(size_t)size)return 1;fclose(f);
 char hex[34]={0};f=fopen(argv[2],"r");if(!f||!fgets(hex,sizeof hex,f))return 1;fclose(f);uint8_t key[16],iv[16]={0};for(int i=0;i<16;i++){unsigned n;if(sscanf(hex+i*2,"%2x",&n)!=1)return 1;key[i]=(uint8_t)n;}
 struct AVAESCTR*ctr=av_aes_ctr_alloc();if(!ctr||av_aes_ctr_init(ctr,key))return 1;av_aes_ctr_set_full_iv(ctr,iv);av_aes_ctr_crypt(ctr,mem.data,mem.data,size);av_aes_ctr_free(ctr);memset(key,0,sizeof key);memset(hex,0,sizeof hex);
 AVFormatContext*fmt=avformat_alloc_context();uint8_t*buffer=av_malloc(32768);AVIOContext*io=avio_alloc_context(buffer,32768,0,&mem,read_data,NULL,seek_data);fmt->pb=io;fmt->flags|=AVFMT_FLAG_CUSTOM_IO;
 if(avformat_open_input(&fmt,NULL,NULL,NULL)<0||avformat_find_stream_info(fmt,NULL)<0)return 1;
 int stream=av_find_best_stream(fmt,AVMEDIA_TYPE_AUDIO,-1,-1,NULL,0);if(stream<0)return 1;AVStream*s=fmt->streams[stream];const AVCodec*codec=avcodec_find_decoder(s->codecpar->codec_id);if(!codec)return 1;
 AVCodecContext*dec=avcodec_alloc_context3(codec);if(avcodec_parameters_to_context(dec,s->codecpar)<0||avcodec_open2(dec,codec,NULL)<0)return 1;
 double offset=atof(argv[3]);if(offset<0)offset=0;if(offset>0)avformat_seek_file(fmt,-1,INT64_MIN,(int64_t)(offset*AV_TIME_BASE),(int64_t)(offset*AV_TIME_BASE),0);
 SwrContext*swr=NULL;AVChannelLayout out=AV_CHANNEL_LAYOUT_STEREO;if(swr_alloc_set_opts2(&swr,&out,AV_SAMPLE_FMT_S16,44100,&dec->ch_layout,dec->sample_fmt,dec->sample_rate,0,NULL)<0||swr_init(swr)<0)return 1;
 AVPacket*pkt=av_packet_alloc();AVFrame*frame=av_frame_alloc();double gain=0.25;long frames=0;
 while(running&&av_read_frame(fmt,pkt)>=0){if(pkt->stream_index!=stream){av_packet_unref(pkt);continue;}if(avcodec_send_packet(dec,pkt)<0){av_packet_unref(pkt);continue;}av_packet_unref(pkt);
  while(running&&avcodec_receive_frame(dec,frame)>=0){
   double t=frame->best_effort_timestamp==AV_NOPTS_VALUE?offset:frame->best_effort_timestamp*av_q2d(s->time_base);if(t+frame->nb_samples/(double)dec->sample_rate<offset){av_frame_unref(frame);continue;}
   int cap=(int)av_rescale_rnd(swr_get_delay(swr,dec->sample_rate)+frame->nb_samples,44100,dec->sample_rate,AV_ROUND_UP);uint8_t*data=NULL;if(av_samples_alloc(&data,NULL,2,cap,AV_SAMPLE_FMT_S16,0)<0)return 1;
   int n=swr_convert(swr,&data,cap,(const uint8_t**)frame->extended_data,frame->nb_samples);if(n<0)return 1;int skip=t<offset?(int)((offset-t)*44100):0;if(skip>n)skip=n;
   int duck=0;f=fopen(argv[4],"r");if(f){duck=fgetc(f)=='1';fclose(f);}double target=duck?0.10:0.25;int16_t*pcm=(int16_t*)data;
   for(int i=skip;i<n;i++){gain+=(target-gain)/2205.;for(int c=0;c<2;c++){int sample=(int)(pcm[i*2+c]*gain);if(sample>8192)sample=8192;if(sample< -8192)sample=-8192;uint16_t v=(uint16_t)(int16_t)sample;data[i*4+c*2]=(uint8_t)(v>>8);data[i*4+c*2+1]=(uint8_t)v;}}
   if(fwrite(data+skip*4,4,n-skip,stdout)!=(size_t)(n-skip)){running=0;}frames+=n-skip;av_freep(&data);av_frame_unref(frame);
  }
 }
 fprintf(stderr,"content reader stopped: %ld output frames; speech is absent from this source\n",frames);av_frame_free(&frame);av_packet_free(&pkt);swr_free(&swr);avcodec_free_context(&dec);avformat_close_input(&fmt);avio_context_free(&io);av_free(mem.data);return 0;
}
