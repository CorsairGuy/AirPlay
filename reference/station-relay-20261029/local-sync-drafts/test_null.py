"""Pi-only safe smoke test: existing image, no ALSA devices, explicit null PCM."""
import pathlib, struct, subprocess, time
base=pathlib.Path(__file__).resolve().parent
stats=base/'null.stats'
stats.unlink(missing_ok=True)
p=subprocess.Popen(['docker','run','--rm','-i','--network','none','--platform','linux/arm/v7',
    '-v',f'{base}:/work','mini2-airplay-armhf','/work/local_player.armhf','null','/work/null.stats'],stdin=subprocess.PIPE)
try:
    # Null has no physical clock/delay: target now-10ms tests protocol and heartbeat,
    # and intentionally does not claim the physical-device drift behaviour.
    for _ in range(300):
        t=time.monotonic_ns()-10_000_000
        p.stdin.write(b'MLP1'+struct.pack('<qI',t,352)+bytes(352*4))
        p.stdin.flush()
        time.sleep(352/44100)
    p.stdin.close()
    assert p.wait(timeout=5)==0
    values=dict(item.split('=',1) for item in stats.read_text().split())
    assert int(values['records'])>200
    assert int(values['start_ticks'])>0
    assert int(values['mono_ms'])>=int(values['ready_after_ms'])
    assert time.monotonic()*1000-int(values['mono_ms'])<2000
    assert abs(float(values['error_ms']))<20
    print('PASS: ARMHF null PCM, 300 timestamped zero records; atomic fresh identity/heartbeat; EOF exit')
    print(stats.read_text().strip())
finally:
    if p.poll() is None: p.kill(); p.wait()
