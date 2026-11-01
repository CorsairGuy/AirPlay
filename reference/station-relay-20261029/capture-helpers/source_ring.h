#ifndef MINI2_SOURCE_RING_H
#define MINI2_SOURCE_RING_H
#include <stdint.h>
#define TAP_FRAMES 131072u
#define TAP_MAGIC 0x41554432u
#define TAP_AIRPLAY_KEY 0x415031
#define TAP_BLUETOOTH_KEY 0x425431
struct SourceRing{volatile uint32_t magic;uint32_t version,pid,rate,channels,reserved[3];volatile uint32_t written;uint32_t pad[7];int16_t samples[TAP_FRAMES*2];};
#endif
