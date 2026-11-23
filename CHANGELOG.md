# Changelog

## Unreleased

- Added configurable AirPlay feature mask, status flags, and accessory type to the receiver. Device profiles are chosen by the caller, independently of any Windows application.
- Investigated approximately 7.4–8.4 s between an observed mDNS query and RTSP startup. In the tested TV receiver, clearing feature bit 48 while retaining bits 40, 41, and 47 (`0x527FFEE6,0x8300`) produced immediate video with a TV icon; repeated connections after resetting iPhone Wi-Fi also succeeded. Restoring bit 48 while clearing bit 47 restored the delay. These observations identify a trigger in this profile, not the iPhone's internal mechanism.
- A pre-RTSP PTP probe was not observed on the tested PC interface; toggling bit 41 did not remove the delay. Removing buffered-audio bit 40 made video fail, so it is not a viable fix. Changing source version (550.10, 220.68, 377.40.00) or omitting TXT `at` did not yield a reported speedup. In one capture, no PC mDNS reannouncement was observed in the pause and Fast/TV PTR, SRV, and TXT TTL and cache-flush settings matched. These network observations are limited to traffic visible to the PC; they do not establish AWDL behavior.
- Open: why iPhone waits when bit 48 is advertised in this profile. The successful mirror session selected NTP even though bit 41 remained advertised; actual PTP operation and relay timing remain separate work.
