`tiny.ts` is a two-frame synthetic H.264 MPEG-TS fixture, generated with:

```sh
ffmpeg -f lavfi -i 'testsrc2=s=64x64:r=5:d=0.4' -an -c:v libx264 -preset ultrafast -f mpegts tiny.ts
```
