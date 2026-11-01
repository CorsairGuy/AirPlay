"""Mock-only lifecycle checks; no hardware controls or Station access."""
import os, pathlib, subprocess, tempfile, time
base=pathlib.Path(__file__).resolve().parent
def ticks(pid):
    return pathlib.Path(f'/proc/{pid}/stat').read_text().rsplit(') ',1)[1].split()[19]
def case(failure):
    with tempfile.TemporaryDirectory(dir=base,prefix='guard-test-') as tmp:
        d=pathlib.Path(tmp); (d/'verified-music').touch()
        mixer=d/'mock-mixer'; mixer.write_text('#!/bin/sh\nprintf "%s\\n" "$*" >> "'+str(d/'calls')+'"\n'); mixer.chmod(0o700)
        sender=subprocess.Popen(['sleep','20']); player=subprocess.Popen(['sleep','20'])
        stats=d/'stats'; now=int(time.monotonic()*1000)
        boot=int(time.clock_gettime(time.CLOCK_BOOTTIME)*1000)
        stats.write_text(f'pid={player.pid} start_ticks={ticks(player.pid)} mono_ms={now} boot_ms={boot} ready=1 ready_after_ms={now-100} records=100 error_ms=0\n')
        env=os.environ|{'MINI2_GROUP_RUN':str(d),'MINI2_GROUP_MIXER':str(mixer)}
        guard=subprocess.Popen(['sh',str(base/'group-sync-guard.sh'),'arm',str(sender.pid),'music',str(stats)],env=env,stdout=subprocess.PIPE)
        try:
            deadline=time.monotonic()+3
            while not (d/'owner-music/active').exists():
                assert guard.poll() is None
                assert time.monotonic()<deadline
                time.sleep(.02)
            if failure=='sender': sender.terminate(); sender.wait()
            elif failure=='player': player.terminate(); player.wait()
            elif failure=='guard': guard.terminate()
            assert guard.wait(timeout=4) in (0,6)
            calls=(d/'calls').read_text().splitlines()
            assert calls==['-q -c 0 sset Mini2 Music 0%','-q -c 0 sset Mini2 Music 100%'],calls
            assert not (d/'owner-music').exists()
            print('PASS guard:',failure,'=> restore + owner cleanup')
        finally:
            for p in (guard,sender,player):
                if p.poll() is None: p.terminate()
                try:p.wait(timeout=2)
                except subprocess.TimeoutExpired:p.kill();p.wait()
for failure in ('sender','player','guard'):case(failure)
