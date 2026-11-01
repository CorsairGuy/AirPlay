"""Offline patches only. Run on Pi; input files are never changed.

python3 prepare_local_sync.py /root/work/claude-pr1 /root/work/codex-routes OUT
"""
from pathlib import Path
import sys

def replace(s, a, b):
    if s.count(a) != 1:
        raise RuntimeError('patch precondition failed: ' + a[:100])
    return s.replace(a, b)

def main(claude, current, out):
    out.mkdir(parents=True, exist_ok=True)
    s = (claude/'src/local_player.c').read_text()
    s = replace(s, '#include <time.h>', '#include <time.h>\n#include <unistd.h>')
    s = replace(s, 'static volatile sig_atomic_t running = 1;', '''static volatile sig_atomic_t running = 1;
// Atomic heartbeat; not proof of acoustic synchronization. Reader/controller must
// additionally verify receiver media timeline and that this PCM routes correctly.
static unsigned long long proc_start(void) {
  FILE *f=fopen("/proc/self/stat", "r"); char b[4096];
  if (!f) return 0;
  if (!fgets(b,sizeof b,f)) { fclose(f); return 0; }
  fclose(f); char *p=strrchr(b, ')'); if (!p) return 0;
  p+=2; char *save=NULL,*v=strtok_r(p," ",&save);
  for(int n=3;v && n<22;n++) v=strtok_r(NULL," ",&save);
  return v ? strtoull(v,NULL,10) : 0;
}
static void report(const char *path, long records, long dropped, long repeated,
                   long delay, long long now, long long ready_after,
                   double error_ms, double worst_ms) {
  char tmp[4096];
  if (snprintf(tmp,sizeof tmp,"%s.%ld.tmp",path,(long)getpid()) >= (int)sizeof tmp) return;
  FILE *f=fopen(tmp,"w"); if (!f) return;
  struct timespec bt; clock_gettime(CLOCK_BOOTTIME,&bt);
  long long boot_ms=(long long)bt.tv_sec*1000+bt.tv_nsec/1000000;
  int ok=fprintf(f,"pid=%ld start_ticks=%llu mono_ms=%lld boot_ms=%lld ready=%d ready_after_ms=%lld records=%ld error_ms=%.2f worst_ms=%.2f dropped=%ld repeated=%ld delay_frames=%ld\\n",
    (long)getpid(),proc_start(),now/1000000LL,boot_ms,now>=ready_after,ready_after/1000000LL,
    records,error_ms,worst_ms,dropped,repeated,delay)>=0;
  if (fclose(f)!=0) ok=0;
  if (ok) rename(tmp,path); else unlink(tmp);
}''')
    s = replace(s, 'int64_t last_report = 0, worst = 0;', 'int64_t last_report = 0, worst = 0, ready_after = 0;')
    s = replace(s, 'records++;', '''records++;
    // First record can write lead-in silence before its content starts.
    if (!ready_after) ready_after=play_at + (int64_t)frames*1000000000LL/RATE;''')
    a = '''if (argc > 2 && t - last_report > 2000000000LL) {
      FILE *f = fopen(argv[2], "w");
      if (f) {
        fprintf(f, "records=%ld error_ms=%.2f worst_ms=%.2f dropped=%ld repeated=%ld delay_frames=%ld\\n",
                records, error / 1e6, worst / 1e6, dropped, repeated, (long)delay);
        fclose(f);
      }'''
    b = '''if (argc > 2 && t - last_report > 200000000LL) {
      report(argv[2],records,dropped,repeated,(long)delay,t,ready_after,error/1e6,worst/1e6);'''
    s = replace(s,a,b)
    # TERM/EOF should not drain an audible tail after restoring the direct route.
    s = replace(s,'snd_pcm_drain(pcm);','snd_pcm_drop(pcm);')
    (out/'local_player.c').write_text(s)
    for name in ('content_reader.c','source_reader.c'):
        s=(current/name).read_text()
        if name=='content_reader.c':
            s=replace(s,'double gain=0.25;', 'double gain=1.0;')
            s=replace(s,'double target=duck?0.10:0.25;', 'double target=duck?0.4:1.0;')
            s=replace(s,'if(sample>8192)sample=8192;if(sample< -8192)sample=-8192;',
                      'if(sample>32767)sample=32767;if(sample< -32768)sample=-32768;')
        else:
            s=replace(s,'double gain=.25;', 'double gain=1.0;')
            s=replace(s,'double target=duck?.10:.25;', 'double target=duck?.4:1.0;')
            s=replace(s,'if(v>8192)v=8192;if(v< -8192)v=-8192;',
                      'if(v>32767)v=32767;if(v< -32768)v=-32768;')
        (out/name).write_text(s)

if __name__=='__main__':
    main(*(Path(x) for x in sys.argv[1:]))
