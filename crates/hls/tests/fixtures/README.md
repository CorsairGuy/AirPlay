`tiny.ts` is a two-frame synthetic H.264 MPEG-TS fixture, generated with:

```sh
ffmpeg -f lavfi -i 'testsrc2=s=64x64:r=5:d=0.4' -an -c:v libx264 -preset ultrafast -f mpegts tiny.ts
```

`av/` is a three-second H.264 + AAC HLS VOD used by the playback tests: three
one-second MPEG-TS segments and their media playlist. The tests serve it from a
loopback HTTP server; nothing is fetched from the network. It was generated
with:

```sh
ffmpeg -f lavfi -i 'testsrc2=s=160x90:r=24:d=3' \
  -f lavfi -i 'sine=frequency=440:sample_rate=48000:duration=3' \
  -c:v libx264 -preset ultrafast -g 12 -keyint_min 12 -sc_threshold 0 -pix_fmt yuv420p \
  -c:a aac -b:a 64k -ac 2 \
  -f hls -hls_time 1 -hls_list_size 0 \
  -hls_segment_filename 'av/seg%d.ts' av/playlist.m3u8
```

The tests treat `localhost` as an allowed host: the production `Url` parser
rejects private IP literals, and `Fetcher::for_tests` (test builds only) omits
the public-DNS resolver so a loopback server is reachable.
