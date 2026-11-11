# AirPlay 1/2 — консолидированный wire reference

<!-- current-status-20261112 -->
> Актуальность12ноября2026: это reference публично исследованных wire profiles и исторических mappings, не полная спецификация Apple. Текущее состояние кода — [STATUS](STATUS.md). Закреплённые источники и даты наблюдений ниже сохранены, AppendixA относится к snapshot2b89119.


Дата: 30 октября 2026. Это авторская спецификация **известных публичных профилей**, восстановленная по первичным исходникам. Она не является полной официальной спецификацией Apple, сертификационным документом или обещанием совместимости со всеми версиями iOS/tvOS. Неизвестные части перечислены явно. Список проектов сам по себе не считается спецификацией: далее приведены структуры, байтовые форматы и переходы состояния.

Точные источники закреплены в ссылках. Основной локально сверенный core snapshot: `2b8911990b3c05f8adf8de3ea29c2e491eb01141`; незакоммиченные sender/packetizer дополнения не используются как нормативное свидетельство. Реализация конкретного приложения отделяется от wire semantics. Обозначения: **RE** — непосредственно реализованный reverse-engineered профиль; **OBS** — ограниченное наблюдение; **UNKNOWN** — данных недостаточно; **POLICY** — рекомендуемая защита реализации, не константа Apple.

## 1. Соглашения, роли и состояния

`C` — controller/sender, `R` — receiver/accessory. Контроллер и источник media могут быть разными узлами. Порты берутся из DNS-SD и SETUP; 7000 — распространённое, но не обязательное значение. Диапазон `[a..b)` исключает байт b. BE/LE обозначают порядок байт; integer в plist не имеет фиксированной текстовой ширины.

| Роль | Вход | Выход / ответственность |
|---|---|---|
| Receiver | discovery requests, RTSP/HTTP, media | capabilities, authentication, negotiated ports, timed rendering, state |
| Sender | source PCM/compressed media, receiver capabilities | session setup, encrypted packets, anchors, teardown |
| Controller | subscribed player/output state | commands, queue requests, group edits; не обязательно media |
| Clock participant | NTP/PTP samples | соответствие remote clock и local monotonic time |
| Group coordinator | member/role/session changes | общий clock/membership; галочки интерфейса не заменяют transport |

Состояния ниже логические: это минимальная согласованная модель, а не утверждение о внутреннем автомате Apple.

| Состояние | Событие | Следующее / результат |
|---|---|---|
| Discovered | TCP к advertised port | Connected; это ещё не authenticated |
| Connected | согласованный pair-setup / pair-verify | Authenticated; при HAP включается control encryption после финального plaintext ответа |
| Authenticated | initial SETUP без streams | SessionReady: event/timing channels, identity/session fields |
| SessionReady | SETUP streams[] | StreamsReady: data/control ports и stream-specific context |
| StreamsReady | RECORD / valid rate+anchor | Playing либо ready-to-render; ACK не доказывает доставку звука |
| Playing | rate=0 / flush range | Paused либо invalidated buffer interval; session может сохраниться |
| Любое negotiated | stream TEARDOWN | закрытие указанных streams, не обязательно доверия/pairing |
| Любое connected | full TEARDOWN / TCP EOF / auth failure | release owned resources; persistent pairing store отдельно |

Classic профиль: ANNOUNCE/SDP → SETUP transport → RECORD → RTP/timing → FLUSH/TEARDOWN. AP2: pairing → initial plist SETUP → stream plist SETUP → RECORD/anchors/media. Control-only AP2 может иметь timingProtocol=None и stream130 без audio. Порядок RECORD и stream130 в pyatv отличается от audio-пути: это не одна универсальная линейная последовательность. [C-SESSION] [P-AP2]

## 2. Discovery, TXT и capabilities

DNS-SD публикует `_airplay._tcp.local` и legacy `_raop._tcp.local`; advertised TCP port относится к control service. RAOP instance традиционно `HEXDEVICEID@Name`. MAC-образный ID — исторический профиль, не универсальный постоянный идентификатор современных Apple TV. [A-DISC]

| TXT key | Wire text / значение | Назначение и ограничения |
|---|---|---|
| features / RAOP ft | `0xLOW32,0xHIGH32`, HIGH optional | mask = low OR (high<<32), **low идёт первым** |
| model / RAOP am | UTF-8 string | device class/model; не доказательство реализованных функций |
| srcvers / RAOP vs | version string | влияет на negotiation вместе с mask; не сравнивать как float |
| protovers | string, например `1.1` | protocol generation hint |
| flags / RAOP sf | hexadecimal status bits | access/pairing state; не то же самое, что features |
| pi, psi | UUID-like text | pairing/system identities; не обязаны совпадать |
| gid, pgid | UUID-like text | group / parent group context |
| igl, gcgl, pgcgl | `0`/`1` text | leader / discoverable leader presence; динамические роли |
| pk | hex public-key text | public identity key; в /info тот же материал может быть plist data |
| acl, pw | decimal/string boolean | access policy; интерпретация зависит от профиля |
| cn / et / md | comma-separated integers | codec / encryption / metadata capability lists |
| sr, ss, ch / tp | decimal / `UDP` или `TCP` | legacy sample rate/bit depth/channels/transport hints |

| Bit index | Публично используемое имя | Достоверность имени |
|---|---|---|
| 0 / 1 / 7 / 9 | URL video / photo / screen mirroring / audio | устойчивые RE названия, разные режимы |
| 11 / 14 / 27 | audio redundancy / FairPlay auth / legacy pairing | RE; bit27 относится к выбору pairing, не к запрету AP2 |
| 15 / 16 / 17 | metadata artwork / progress / text в airplay-spec | core называет bit16 METADATA_ARTWORK: **расхождение**, не исправлено догадкой |
| 18–22 | audio-format capabilities | не приравнивать автоматически к ct или одному codec |
| 32 | IsCarPlay / SupportsVolume | спорное/перегруженное имя |
| 38 | UnifiedMediaControl либо CoreUtils pairing | названия источников расходятся |
| 40 / 41 / 42 | BufferedAudio / PTP / ScreenMultiCodec | RE; наличие advertised bit требует backend |
| 43 / 46 / 47 / 48 | SystemPairing / HK pairing / transient-or-system / CoreUtils-or-transient | 47/48 и часть названий версионно расходятся |
| 51 | UnifiedPairSetupAndMFi | не означает наличие MFi authentication hardware/software |

Сохранять numeric masks и неизвестные bits. `0x527FFEE6,0` не содержит high bits40/41/46/48; это legacy-video профиль, а не доказанный единственный mask для iOS18. Текущий UxPlay по умолчанию имеет `0x5A7FFEE6,0` с bit27 ON. [A-FEATURES] [C-DEVICE] [U-DNSSD]

GET /info → 200, Content-Type `application/x-apple-binary-plist`; request body тоже может быть plist с qualifiers. Проверять decoded types, не XML-представление двоичного plist.

| /info field | Plist type | Направление / смысл |
|---|---|---|
| deviceID, macAddress, name, model, pi | string | R→C identity/name; необязательность отдельных IDs зависит от версии |
| features, statusFlags | integer | R→C capability/status masks, не строка TXT |
| pk | data | R→C public key; не session secret |
| sourceVersion, protocolVersion | string | R→C; соответствуют version hints |
| audioFormats | array<dict> | type, audioInputFormats/audioOutputFormats bitmasks |
| audioLatencies | array<dict> | type/audioType/inputLatencyMicros/outputLatencyMicros |
| displays | array<dict> | uuid,widthPixels,heightPixels,maxFPS,features; UxPlay также dimensions/refreshRate |
| playbackCapabilities | dict | например supportsUIForAudioOnlyContent:boolean; profile-specific |
| igl/gcgl/gid/psi | bool/string, когда присутствуют | core публикует; **не обязательны top-level у всех Apple TV** |

`hasUDPMirroringSupport` и `featuresEx` встречаются в переданном Apple TV наблюдении; полный формат/семантика здесь UNKNOWN. Нельзя объявлять UDP mirroring, имея только TCP receiver. [C-DEVICE] [U-HANDLERS]

## 3. RTSP/HTTP envelope и операции

Request: `METHOD SP URI SP RTSP/1.0 CRLF`; некоторые endpoints используют HTTP/1.1. Response: `VERSION SP status SP reason CRLF`. Далее case-insensitive headers, пустая CRLF строка и ровно Content-Length байт body. Content-Length считает байты, не Unicode characters. TCP read не равен сообщению; остаток следующего сообщения сохраняется. [C-RTSP]

| Header | Тип / связь |
|---|---|
| CSeq | decimal request correlation; response повторяет значение |
| Content-Length / Content-Type | byte count / MIME; не угадывать codec по URI |
| Session | negotiated/legacy session identifier; не groupUUID |
| Transport | legacy RTP/AVP/UDP и параметры ports; не AP2 streams plist |
| RTP-Info | `seq=DEC;rtptime=DEC`; последовательность 16bit, timestamp32bit |
| DACP-ID / Active-Remote | legacy source-control correlation; не authentication session key |
| X-Apple-HKP | `4` в HAP transient sender profile |
| X-Apple-ProtocolVersion / User-Agent | negotiation/debug context, не нормативные возможности |

| Метод / path | Body и направление | Успех / ошибочная трактовка |
|---|---|---|
| OPTIONS * | C→R, обычно empty | 200 + Public methods; не подтверждает все заявленные пути |
| GET /info | C→R, optional plist | 200 plist; malformed/unsupported по реализации |
| POST /pair-pin-start | C→R empty | initiates pairing UX/profile; не универсально обязателен |
| POST /pair-setup, /pair-verify | C→R TLV8 либо отдельный legacy format | state-machine reply; HAP error может быть внутри HTTP200 |
| POST /fp-setup | C→R octet-stream | FPLY response; неподдержанный challenge не равен общему запрету video |
| ANNOUNCE sessionURI | C→R application/sdp | 200 при accepted classic stream; AP2 plist receiver может не реализовывать |
| SETUP sessionURI | C→R Transport header либо binary plist | 200 ports/plist; неподдержанный формат явно отвергать |
| RECORD / FLUSH / TEARDOWN | C→R session context | RECORD ready; FLUSH invalidates pre-announced media; full teardown releases owned session |
| SETRATEANCHORTIME / FLUSHBUFFERED | C→R binary plist | anchor/range operations, см. ниже; 200 без действия не доказывает реализацию |
| SETPEERS / SETPEERSX | C→R plist | clock/peer/group context; layouts зависят от профиля |
| GET_PARAMETER / SET_PARAMETER | text/parameters, DMAP, image | volume/progress/text/art; см. metadata |
| POST /feedback | C→R, часто empty | keepalive; интервал 2s — pyatv profile, не глобальный timeout |
| POST /command | plist, receiver→source по event channel или C→R | отдельный command namespace, не protobuf MRP command enum |

HTTP/RTSP status и HAP/MRP errorCode — разные слои. Core использует 400 для bad plist/FPLY,500 для stream setup failure,501 для неизвестной операции; это **поведение core**, не универсальная таблица Apple errors. После AEAD authentication failure нельзя продолжать как будто байт был пропущен: потеряна защищённая последовательность. [C-SESSION] [C-MEDIA]

## 4. HAP authentication и control encryption

Не смешивать HAP TLV8, legacy pairing и FairPlay. В этом разделе описан реализованный HAP профиль; legacy binary pair-verify и MFi auth не объявляются его вариантами. Источники: [C-HAP] [C-TLV] [C-SRP] [C-KDF] [P-AP2].

TLV8 item: `[type:u8][length:u8][value:length bytes]`. Значение длиннее 255 делится на соседние items того же type; продолжение после полного 255-byte фрагмента склеивается. Не превращать любые повторные одинаковые types в одно значение независимо от границ.

| Type | Имя | Wire value |
|---|---|---|
| 0x00 / 0x01 | Method / Identifier | small integer / identifier bytes |
| 0x02 / 0x03 / 0x04 | Salt / PublicKey / Proof | bytes; SRP key384bytes в 3072bit profile, X25519 key32 в verify |
| 0x05 / 0x06 / 0x07 | EncryptedData / State / Error | ciphertext+tag / LE integer / error integer |
| 0x0A / 0x13 | Signature / Flags | Ed25519 signature64 / LE flags; transient=0x10 |

SRP-6a profile: RFC5054 group3072, g=5, SHA-512, username ASCII `Pair-Setup`; transient PIN ASCII `3939`, persistent PIN берётся из политики receiver. Salt16bytes в core. Fresh random private values нужны при каждом новом exchange. Public integer wire encoding3072bit padded; арифметика moduloN. Core independently tests against srptools. Отклонять A modN=0/B modN=0, неверные proofs и неожиданное состояние.

| Шаг | Direction | TLV содержимое / переход |
|---|---|---|
| Setup M1 | C→R | State1, Method0, optional Flags0x10 |
| Setup M2 | R→C | State2, Salt, server SRP PublicKeyB |
| Setup M3 | C→R | State3, client PublicKeyA, client Proof |
| Setup M4 | R→C | State4, server Proof; transient заканчивается здесь |
| Setup M5 | C→R persistent | State5, EncryptedData: Identifier, Ed25519 PublicKey, Signature |
| Setup M6 | R→C persistent | State6, EncryptedData с accessory Identifier/PublicKey/Signature; pairing сохраняется |
| Verify M1 | C→R paired | State1, ephemeral X25519 PublicKeyC |
| Verify M2 | R→C | State2, ephemeral PublicKeyR, EncryptedData(IdentifierR,SignatureR) |
| Verify M3 | C→R | State3, EncryptedData(IdentifierC,SignatureC) |
| Verify M4 | R→C | State4; после plaintext ответа включить encrypted control |

Setup proof использует общий SRP K=SHA512(S). Persistent signing input: HKDF32(K, соответствующий Sign-Salt/Info) || Identifier || Ed25519PublicKey. В verify signatureR подписывает R_ephemeral || IdentifierR || C_ephemeral, signatureC — C_ephemeral || IdentifierC || R_ephemeral. Verify shared=raw X25519 DH result, а transient shared=SRP session key: это разные типы IKM. Error2 означает authentication rejection в проверенном core; полный набор HAP error codes/permissions здесь не восстановлен.

Все строки следующей таблицы ASCII, с точным регистром. HKDF=HKDF-SHA512, output32bytes. ChaCha20-Poly1305 nonce12bytes: четыре нуля || 8-byte suffix. Setup/verify sub-TLV имеет empty AAD и ASCII nonce suffix, channel records — counter nonce и length AAD.

| Purpose | Salt | Info / direction либо nonce suffix |
|---|---|---|
| Setup encrypted sub-TLV | Pair-Setup-Encrypt-Salt | Pair-Setup-Encrypt-Info; PS-Msg05 / PS-Msg06 |
| Setup controller signature seed | Pair-Setup-Controller-Sign-Salt | Pair-Setup-Controller-Sign-Info |
| Setup accessory signature seed | Pair-Setup-Accessory-Sign-Salt | Pair-Setup-Accessory-Sign-Info |
| Verify encrypted sub-TLV | Pair-Verify-Encrypt-Salt | Pair-Verify-Encrypt-Info; PV-Msg02 / PV-Msg03 |
| Control C→R | Control-Salt | Control-Write-Encryption-Key |
| Control R→C | Control-Salt | Control-Read-Encryption-Key |
| Event R→C | Events-Salt | Events-Write-Encryption-Key |
| Event C→R | Events-Salt | Events-Read-Encryption-Key |
| Data130 C→R | DataStream-Salt{seed unsigned decimal} | DataStream-Output-Encryption-Key |
| Data130 R→C | DataStream-Salt{seed unsigned decimal} | DataStream-Input-Encryption-Key |

Control/event/data encrypted record: `[plaintextLength:u16LE][ciphertext:N][tag:16]`. AAD=исходные 2bytes length, nonce suffix=64bit LE record counter, начинается 0 отдельно для каждого направления и канала; N≤1024 в HAP channel profile. TCP fragments/coalescing не сбрасывают counter. После reconnect нельзя повторить key+counter; нужен новый согласованный key context. Plist/MRP framing находится **внутри** этих records и может пересекать их границы. [C-CHANNEL]

## 5. Classic audio: SDP, transport и RTP

ANNOUNCE application/sdp задаёт session и codec. SDP lines CRLF, ключи/параметры — текст. Минимальный ALAC profile ниже использует placeholders вместо реальных идентификаторов/ключей. [A-ANNOUNCE]

```text
v=0
o=Sender SESSION_ID 0 IN IP4 SENDER_IP
s=AirTunes
c=IN IP4 RECEIVER_IP
t=0 0
m=audio 0 RTP/AVP 96
a=rtpmap:96 AppleLossless
a=fmtp:96 352 0 16 40 10 14 2 255 0 0 44100
a=rsaaeskey:BASE64_WRAPPED_KEY
a=aesiv:BASE64_IV
```

`fpaeskey` — другой wrapped-key profile, не синоним `rsaaeskey`. ALAC fmtp example задаёт frameLength352, bitDepth16, channels2, sampleRate44100; прочие числовые ALAC config параметры нельзя заменять по guessed codec. AAC использует `mpeg4-generic/44100/2`; AAC-ELD example имеет mode=AAC-eld и constantDuration480. Presence/тип key wrapping согласуются с advertised auth; RSA и FairPlay имеют разные exchange paths.

Legacy SETUP Transport example: `RTP/AVP/UDP;unicast;mode=record;control_port=C;timing_port=T`; R отвечает server_port(audio), control_port, timing_port. Наличие слова `interleaved=0-1` в некоторых historical examples **не означает**, что эта UDP media автоматически стала RTP-over-RTSP/TCP. Другие TCP classic профили здесь не специфицированы. [A-SETUP]

| RTP offset | Bytes | Формат для fixed-header audio profile |
|---|---|---|
| 0 | 1 | version2 встарших 2bits; обычный byte0x80; CSRC/extensions не задействованы этим профилем |
| 1 | 1 | marker в bit7, payloadType в bits0..6; audio PT96=0x60, first packet0xE0 |
| 2..4 | 2 | sequence u16BE, modulo2^16 |
| 4..8 | 4 | RTP sample-frame timestamp u32BE, modulo2^32 |
| 8..12 | 4 | SSRC u32BE, session-specific |
| 12..end | variable | codec payload; encryption зависит от agreed profile |

Timestamp увеличивается на sample frames, не bytes и не число RTP packets. Потерянные/пропущенные source frames сохраняют разрыв timeline. Для PCM S16BE stereo один frame=4bytes;352frames=1408bytes — конкретный negotiated profile, не универсальный packet size. Marker отмечает начало/после flush согласно session profile. [C-RTP] [A-RTP]

Legacy AES-128-CBC media profile: decrypt полных 16-byte blocks payload с negotiated IV; неполный хвост может оставаться plaintext. Не распространять это правило на AP2 ChaCha или video AES-CTR. Внутренний nonce/sequence не следует генерировать заново при retransmit: повторяется исходный packet.

| Control packet | Байты и назначение |
|---|---|
| Timing request0x52 / wire0xD2 | 32bytes: header0..4, padding4..8, origin8..16, receive16..24, transmit24..32; timestamps NTP BE |
| Timing reply0x53 / wire0xD3 | origin=request transmit; receive/transmit отражают времена R; correlation по original timestamp |
| Sync0x54 / wire0xD4 | 20bytes: byte0=0x90 first либо 0x80; byte1=0xD4;2..4=0x0007;4..8=RTP reference;8..16=NTP;16..20=current RTP |
| Retransmit request0x55 | ≥8bytes: RTP-like4-byte prefix;4..6=first lost seq u16BE;6..8=count u16BE |
| Retransmit reply0x56 / wire0xD6 | outer4bytes `[0x80,0xD6,seqBE]`, затем исходный audio packet целиком |

Для sync с latencyL frames: reference=currentRTP−L modulo2^32; reference соответствует NTP anchor. Seq retries идут modulo2^16; request header sequence не тот же field, что first lost seq. Cache512 и cap512 ответов — **POLICY/Station sender profile**, не константа wire protocol. NTP/PTP clocks подробнее вразделе 8. [A-RTP] [C-NET] [C-NTP]

## 6. AP2 plist SETUP и streams

Во всех таблицах C→R request и R→C response — binary plist dictionaries, если не указано другое. Первый SETUP без streams задаёт session; следующий с streams array задаёт media/control channels. Не требовать ekey при каждом stream SETUP: он может находиться в initial SETUP/связанной session. [C-MEDIA] [P-AP2]

| Initial field | Plist type | Значение / direction |
|---|---|---|
| deviceID, macAddress, name, model | string | C→R identity/profile; не ключи шифрования |
| sessionUUID | string UUID | C→R отдельная session identity, не groupUUID |
| sourceVersion, osName, osVersion, osBuildVersion | string | C→R source profile hints |
| timingProtocol | string | C→R NTP, PTP либо None |
| timingPort | integer(port) | C→R NTP endpoint; R→C свой negotiated endpoint,0 если unused в некоторых profiles |
| timingPeerInfo | dict {Addresses:array<string>,ID:string} | PTP peer context; не полный group manifest |
| timingPeerList | array / profile-specific structures | peer enumeration; требуется конкретный negotiated layout |
| groupUUID, groupContainsGroupLeader | string UUID, bool | C→R AirPlay group identity/role context |
| isGroupLeader, isMultiSelectAirPlay | bool | C→R role/group-related profile hints |
| isRemoteControlOnly, isScreenMirroringSession | bool | C→R session purpose; самостоятельные признаки |
| ekey, eiv | data72, data16 в legacy-mirror FPLY profile | C→R wrapped key and IV; **не общий ключ для всех AP2 потоков** |
| eventPort | integer(port) | R→C endpoint encrypted event channel |

| Stream type | Request descriptor | Response descriptor / media transport |
|---|---|---|
| 96 realtime audio | type:int96, ct:int, audioFormat:int mask, sr:int, spf:int, optional ss:int, controlPort:int, shk:data32 в ChaCha profile | type96,dataPort,controlPort; negotiated UDP audio + UDP control |
| 103 buffered audio | type103, codec/format parameters, shk:data32; descriptor specifics sender-dependent | type103,dataPort,controlPort,audioBufferSize; TCP buffered media + control channel |
| 110 screen mirror | type110,streamConnectionID:uint64; associated session key context | type110,dataPort; verified legacy-compatible path=TCP video |
| 130 remote control | type130,seed:uint64,controlType:int2,channelID/clientUUID:string,wantsDedicatedSocket:bool,clientTypeUUID:string | type130,streamID:int,dataPort:int при dedicated channel; data130 encryption выше |

`clientTypeUUID=1910A70F-DBC0-4242-AF95-115DB30604E1` — конкретное pyatv control profile, не обязательный UUID любого MRP client. Type130 без seed может быть logical registration без socket в core: не считать streamID подтверждением data connection. [P-AP2] [C-MEDIA]

Compression type `ct`:1PCM,2ALAC,4AAC-LC,8AAC-ELD в проверенных code paths. `audioFormat` — отдельная bitmap/config namespace: пример PCM16BE/44100/stereo=0x800 в sender profile; не приравнивать ct к audioFormat. `sr` frames/sec, `spf` frames/packet, `ss` sample bits. Поддержка mono/surround/24bit/48k зависит от agreed codec+format: AP2 не означает только AAC и не означает все кодеки сразу. [C-MEDIA] [S-AP2]

AP2 audio ChaCha packet: `[RTP header12][ciphertext:N][tag16][nonceSuffix8]`. AEAD nonce=`00000000 || suffix8`; sender profile кодирует suffix как counter u64LE; AAD=RTP bytes4..12(timestamp+SSRC). Seq/marker в AAD этого профиля не входят. На retry возвращать тот же ciphertext/tag/nonce; sequence wrap не сбрасывает nonce. [C-RTP]

Buffered103 TCP framing: `[length:u16BE][audio packet:length−2]`, length включает собственные 2bytes. Это **не** HAP encrypted-control record с LE length и не MRP32-byte frame. Недопустимы length<2 и неполный EOF; устанавливать memory/backpressure limits. Layout codec payload внутри RTP должен соответствовать decoder config, а не просто удалению TCP length prefix. [C-RTP] [C-NET]

## 7. Timing anchors, pause/flush и group context

Anchor описывает пару `(RTP frame, network time, clock timeline ID)` и rate/playing. Формула presentationTime(RTPx)=networkAnchor+(signed_wrap_delta(RTPx,RTPanchor)/sampleRate). Перевод к local monotonic clock требует offset выбранной timeline. Drift correction/output latency — политика renderer, не новое wire поле.

| SETRATEANCHORTIME field | Plist type / единицы |
|---|---|
| rate | real/integer;0pause,1normal в проверенном renderer |
| rtpTime | uint, effective32bit sample-frame timestamp |
| networkTimeSecs | uint seconds |
| networkTimeFrac | uint, доля seconds в units2^-64; **не NTP32bit fraction** |
| networkTimeTimelineID | uint64 clock ID; сопоставить PTP clock context |

Пауза может приходить без нового timestamp и сохранять предыдущий anchor. Core так делает; это не разрешение выдумать anchor при его отсутствии. FLUSHBUFFERED принимает flushUntilTS и optional flushFromTS, RTP32bit интервал. Иные поля seq/flush transition variants не восстановлены исчерпывающе; нельзя выдавать core subset за всю semantics Apple. [C-MEDIA]

NTP timestamp=seconds1900 inhigh32 | fractional seconds inlow32, BE8bytes. NTP estimate: offset=((t2−t1)+(t3−t4))/2; rtt=(t4−t1)−(t3−t2). Требуется обрабатывать timestamp era/overflow явно; wall-clock jumps не должны разрушать monotonic playout. [C-NTP]

PTP subset: UDP319 event/320general, header34bytes; messageType low nibble byte0, versionPTP low nibble byte1=2; correctionField bytes8..16=signedBE scaled nanoseconds(>>16); sourceClockIdentity20..28. Sync kind0,Follow_Up8,Announce0xB; preciseOriginTimestamp в Follow_Up после header: seconds48bitBE + nanoseconds32bitBE. Clock ID не является NTP seconds и не deviceUUID. [C-PTP]

Core tracks Announce source/grandmaster and Follow_Up offset; это receiver subset, не доказательство полноценного IEEE1588 BMCA/grandmaster implementation. NQPTP тоже явно реализует часть PTP. Реальная accuracy/link delay и Apple PTP extensions остаются profile-dependent. [N-README]

AP2 control anchor type0x57: ≥28bytes, RTP4..8 u32BE, network nanoseconds8..16 u64BE, clockID20..28 u64BE; bytes16..20 здесь не интерпретированы. Не подставлять в этот anchor NTP32.32 вместо nanoseconds. [C-NET]

SETPEERS/SETPEERSX и groupUUID связывают timing/membership, но не заменяют MRP outputs. Stereo cluster, multiroom group, media-session group и household — разные identity domains. Shared clock ports без валидных anchors не дают synchronized playback. [C-MEDIA]

## 8. Metadata, volume и управление источником

Content-Type запроса SET_PARAMETER определяет формат. DMAP item: `[tag:4 ASCII][length:u32 BE][value:length]`; container содержит вложенные items. Значения строк — UTF-8, без завершающего NUL. Известны minm=название, asar=исполнитель, asal=альбом, mlit=container. Незнакомый tag пропускается по length. Artwork передаётся отдельным image MIME. [A-META] [C-MEDIA]

| Payload | Значение / единицы |
|---|---|
| `volume: FLOAT\r\n` | attenuation dB; диапазон −30..0, mute sentinel −144; это не процент |
| `progress: START/CURRENT/END\r\n` | RTP frame counters; elapsed=(CURRENT−START)/sampleRate, duration=(END−START)/sampleRate, с обработкой wrap |
| RTP-Info rtptime | момент вступления metadata в силу, в sample clock, если присутствует |
| image body | encoded artwork bytes; MIME указывает формат картинки |

MRP volume имеет float 0..1 и endpoint/member UIDs. Преобразование core `v=0 → −144`, иначе `dB=30v−30` — политика конкретной реализации, не универсальная acoustic loudness curve. Hardware gain, group master gain и per-member gain различаются. [A-VOLUME] [C-REMOTE]

Event-channel POST /command использует binary plist: type=`sendMediaRemoteCommand`; modernMediaRemoteCommand=0 play,1 pause,2 toggle,3 stop,4 next,5 previous,24 seek. Seek передаёт params.kMRMediaRemoteOptionPlaybackPosition:real seconds. Профиль volume в core имеет value=`dvlc`, volume:real и params.volume:real. Эти номера **не совпадают** с protobuf MRP Command enum. Legacy DACP использует DACP-ID/Active-Remote и отдельный service; он не обязателен для AP2. [C-REMOTE]

## 9. MRP: framing, envelope, bootstrap и outputs

После расшифровки data130 plaintext содержит 32-byte header и body. Header integers — BE. HAP record limit1024 не ограничивает MRP frame1024 байтами. [C-MRPFRAME] [C-MRPWIRE]

| Offset | Bytes | Значение |
|---|---|---|
| 0..4 | 4 | total length u32 BE, **включает header32** |
| 4..8 | 4 | ASCII sync / rply |
| 8..16 | 8 | reserved, нули в проверенном профиле |
| 16..20 | 4 | cmnd для sync, нули для rply |
| 20..28 | 8 | sequence u64 BE; rply повторяет sequence подтверждаемого sync |
| 28..32 | 4 | reserved, нули |
| 32..end | variable | sync: binary plist `{params:{data:<protobuf packet>}}`; rply: empty |

Каждый sync получает rply независимо от application reply: транспортный ACK не подтверждает выполнение команды. Внутри params.data идут сообщения `[messageLength:varint][ProtocolMessage bytes]`. Bare ProtocolMessage тоже встречается; первый byte0x08 неоднозначен, потому что длина 8 также кодируется 0x08. Существующая эвристика core не считается однозначной wire-спецификацией. Limits4MiB/8192fields/128messages — защитная политика, не доказанные глобальные пределы Apple.

Protobuf key=(fieldNumber<<3)|wireType;0=varint,1=fixed64 LE,2=length-delimited,5=fixed32 LE. Float/double используют IEEE754 bits. Wire type0 сам по себе не сообщает signedness: например hostDeviceClass=−1 нельзя понимать как новый положительный UID/counter. Unknown fields и повторения сохраняются. Appendix содержит **609 mappings среза core**, а не всю семантику MRP. [C-SCHEMA]

| ProtocolMessage field | Tag / type | Назначение |
|---|---|---|
| type | 1 / enum varint | message type, **не extension tag** |
| identifier | 2 / string | request/reply correlation |
| authenticationToken | 3 / string | profile-specific, не публиковать реальные tokens |
| errorCode | 4 / enum varint |0=success; domain отличается от SendError |
| timestamp | 5 / varint | общий смысл и epoch здесь UNKNOWN |
| errorDescription | 78 / string | optional diagnostic |
| uniqueIdentifier | 85 / string | идентичность сообщения, не deviceUID |

| Message type | Extension tag | Direction / назначение |
|---|---|---|
|1 SendCommand /2 Result |6 /7 | C→R command; R→C result |
|4 SetState |9 | R→C player/state/queue |
|15 DeviceInfo /37 DeviceInfoUpdate |20 /20 | bootstrap в обе стороны / device updates |
|16 ClientUpdatesConfig |21 | C→R subscriptions |
|17 VolumeAvailability |22 | R→C capabilities |
|32 PlaybackQueueRequest |37 | C→R queue/assets request |
|38 SetConnectionState |42 | C→R logical connection state |
|46 SetNowPlayingClient /47 Player |50 /51 | R→C active client/player |
|48 ModifyOutputContext |52 | C→R add/remove/set members |
|49 GetVolume /50 Result /51 SetVolume /52 VolumeDidChange |53 /54 /55 /56 | scoped volume query/set/event |
|55 UpdateClient /57 UpdateArtwork |59 /61 | R→C metadata/artwork updates |
|64 VolumeCapabilities /65 UpdateOutputDevice /66 RemoveOutputDevices |68 /69 /70 | R→C endpoint/member state |
|101 DiscoveryMode /102 UpdateEndpoints /103 RemoveEndpoints |82 /83 /84 | endpoint discovery/control |
|120 ConfigureConnection |94 | profile/group hint, не audio SETUP |

Reference bootstrap: DEVICE_INFO request/reply → connection state + updates config → topology + client/player + SetState. После изменения состояния подписчикам идут updates; queue/assets могут запрашиваться повторно без смены трека. Полный порядок всех capability-dependent сообщений UNKNOWN. Reply identifier коррелирует с запросом; unknown command нельзя подтверждать как выполненную. [C-MESSAGES] [C-MRPSERVICE]

| Message / fields | Tags и подтверждённые типы |
|---|---|
| DeviceInfo identity | uniqueIdentifier1:string, name2:string, deviceUID19:string |
| DeviceInfo group | logicalDeviceCount22:uint, isProxyGroupPlayer24:bool, groupUID26:string, groupedDevices28:repeated DeviceInfo, isGroupLeader29:bool |
| DeviceInfo cluster/context | clusterType37:enum, routingContextID41:string, airPlayGroupID42:string, configuredClusterSize51:uint |
| Output identity | name1, uniqueIdentifier2, groupID3:strings; isGroupLeader8:bool; isProxyGroupPlayer22:bool |
| Output context | volume24:float, parentGroupIdentifier34:string, clusterType42:enum, primaryUID43:string, configuredClusterSize44:uint, airPlayGroupID51:string |
| UpdateOutputDevice | endpointUID2:string, clusterAwareOutputDevices3:repeated descriptor |
| RemoveOutputDevices | outputDeviceUIDs1:repeated string, endpointUID2:optional string |
| ModifyOutputContext | type1:enum SharedAudioPresentation=1; adding/removing/settingDevices2/3/4; clusterAwareAdding/Removing/Setting5/6/7:repeated strings |
| SetVolume / VolumeDidChange | set:volume1:float, outputDeviceUID2:string; event:volume1:float, endpointUID2:string, outputDeviceUID3:string |
| SetState / PlayerPath | playbackQueue3:message, playbackState6:enum, playerPath9:message; path origin1/client2/player3:messages |
| ContentItemMetadata | title1/albumName6/trackArtistName7:strings; duration14/elapsedTime35:double; playbackRate39:float; elapsedTimeTimestamp74:double; artworkIdentifier80:string |

Reference elapsedTimeTimestamp использует Cocoa epoch2001-01-01, не NTP epoch; он нужен для экстраполяции elapsedTime. Значения playback/error enums берутся из descriptors, не из похожих modern command numbers. Artwork identifier должен отражать актуальный asset. [C-MESSAGES]

Проверенный числовой subset enums [C-SCHEMA] (это разные namespaces):

| Enum namespace | Numeric values |
|---|---|
| Command |0 Unknown,1 Play,2 Pause,3 TogglePlayPause,4 Stop,5 NextTrack,6 PreviousTrack,45 SeekToPlaybackPosition |
| PlaybackState |0 Unknown,1 Playing,2 Paused,3 Stopped,4 Interrupted,5 Seeking |
| ErrorCode |0 NoError,3 OperationNotPermitted,6 UnsupportedOperation,18 VolumeValueIsOutOfRange,24 EndpointDoesNotExist,28 AddingOrRemovingDevicesFromTheAvOutputContextHasFailed,33 OneOrMoreSpecifiedOutputDevicesAreNotGroupable,39 CouldNotFindTheGivenOutputdevice,100 FailedToConnectToRemoteDevice |
| SendError |0 NoError,1 ApplicationNotFound,2 ConnectionFailed,3 Ignored,4 CouldNotLaunchApplication,5 TimedOut,6 OriginDoesNotExist,7 InvalidOptions,8 NoCommandHandlers,9 ApplicationNotInstalled,10 NotSupported |
| SetConnectionStateMessage |0 None,1 Connecting,2 Connected,3 Disconnected |

UpdateOutputDevice и RemoveOutputDevices — разные messages; полные snapshot/delta semantics всех endpoint scopes не установлены. Конфликт add/remove одного UID, порядок setting/adding и обязательность endpointUID требуют session-specific доказательств. Logical count multiroom leader и physical cluster size — разные сущности. Согласование MRP groupUID, AP2 groupUUID и Bonjour gid проверяется по ролям, а не простым равенством всех UUID.

## 10. Screen mirroring и URL/HLS

Проверенный legacy-compatible mirror: `/fp-setup` challenge16bytes→response142, затем challenge164→response32; сохранённый keymsg164 + SETUP ekey72 дают unwrapped AES seed16. Это FPLY profile UxPlay, не универсальный DRM API. Initial eiv16 относится к связанному audio CBC; video CTR IV выводится отдельно. [U-FPLY] [U-HANDLERS]

Без legacy pair-verify shared session seed равен unwrapped key. При успешном legacy ECDH: seed=SHA512(unwrapped16 || shared32)[0..16], с отдельными исключениями old-client profile. Реальный hash UxPlay — SHA512, несмотря на ошибочный debug-текст sha256. Нельзя подставлять произвольный HAP/SRP secret вместо legacy ECDH. [U-CRYPTO]

Stream key=SHA512(ASCII`AirPlayStreamKey` || unsigned decimal(streamConnectionID) || seed)[0..16]; IV аналогично с `AirPlayStreamIV`. Между частями нет NUL. AES128 CTR state сохраняется на video connection, не сбрасывается на каждый NAL. Это другой cipher/key schedule, чем audio ChaCha/CBC. [U-MIRROR] [C-MIRROR]

| Mirror TCP header offset | Формат проверенного core profile |
|---|---|
|0..4 | payload length u32 LE, исключая header |
|4..6 | kind u16 LE:0/0x1000 frame,1 codec config,2 heartbeat,5 plist; другие варианты UNKNOWN |
|8..16 | presentation timestamp NTP32.32 u64 LE в NTP profile |
|16..128 | остальные поля не полностью специфицированы; не объявлять их обязательно нулевыми |
|128.. | payload; video frame encrypted CTR, codec configuration обрабатывается отдельно |

Decoded frame содержит NAL units с 4-byte BE length (AVCC style), не обязательно AnnexB start codes. H264 configuration использует avcC/SPS/PPS; HEVC требует соответствующей hvcC-related configuration. Полный layout её вариантов не восстановлен. Orientation/resolution change может обновлять configuration. Renderer должен использовать именно согласованную clock domain.

### 10.1. Modern mirroring: известный sender path и граница UNKNOWN

В [doubletake, commit5e2f1cbe](https://github.com/omarroth/doubletake/blob/5e2f1cbe3e1070bcefec8e549c7baf884727bc00/internal/airplay/mirror.go#L1047) есть современный sender path с encrypted HAP control и PTP. Это не только гипотеза о маске, но исполняемые ветки отправителя; их симметричная совместимость с iPhone→наше ядро ещё не проверена.

| Участок | RE: современный путь doubletake |
|---|---|
| Policy | Encrypted control + feature41 + подходящий srcvers выбирают PTP; конкретный version gate — политика этого sender |
| Initial SETUP | Descriptor-only FairPlay placement; legacy root ekey/eiv не обязателен |
| Video stream | type110, streamConnectionID, shk/shiv внутри video descriptor |
| Video key | ChaCha20-Poly1305 key через HKDF-SHA512 из pair-verify X25519 shared secret; предусмотрен fallback на raw FairPlay seed |
| KDF salt/info | DataStream-Salt плюс десятичный videoID; DataStream-Output-Encryption-Key для sender→receiver |
| Screen audio | type96, usingScreen=true; отдельный случайный32-byte shk для audio ChaCha; это не buffered103 |
| Connection shape | feature59 выбирает streamConnections; UseStreamEncryptionKey и isMedia=false относятся к этому audio descriptor |

Не интерпретировать video shk как непосредственно готовый32-byte audio-style ключ: derivation разная. [Crypto selection/KDF](https://github.com/omarroth/doubletake/blob/5e2f1cbe3e1070bcefec8e549c7baf884727bc00/internal/airplay/mirror.go#L1167), [compatibility policy](https://github.com/omarroth/doubletake/blob/5e2f1cbe3e1070bcefec8e549c7baf884727bc00/internal/airplay/compatibility.go#L58).

**UNKNOWN** остаются полная receiver negotiation policy iPhone и одновременные video110 + buffered103 + PTP + multiroom. Одновременное объявление screen и buffered/PTP не доказывает buffered soundtrack при mirroring. UDP mirror, featuresEx и все PTP video/framing variants требуют дополнительных данных. Core ekey-only/NTP-or-NoTime/TCP-only ограничения не являются доказательством запрета протокола. shairplay-rust признал прошлые FairPlay-table ошибки и оставил hybrid profile TODO. [R-STATUS]

Legacy URL video: POST /play принимает text/parameters либо plist Content-Location:URL и Start-Position:real0..1; POST /scrub?position=seconds, POST /rate?value=0|1, POST /stop; GET /scrub возвращает duration/position seconds, GET /playback-info — plist duration/position/rate/readyToPlay/ranges. HLS resource fetch и reverse HTTP/PTTH — отдельные подпроцессы, не stream110. Их современный полный lifecycle здесь UNKNOWN. [A-VIDEO]

## 11. Ошибки, ресурсы и неизвестные части

POLICY для реализации: checked length arithmetic и ARMv7 conversions; bounds на field/frame/count/depth; bounded queues/backpressure; fragmented/coalesced TCP parsing; timeouts; ownership каждой session; запрет key+nonce reuse; unknown-field preservation. Transport ACK не подтверждает acoustic readiness. Реальные ключи и signed media URLs не должны попадать в ordinary logs/fixtures.

**Не полностью специфицированы:** все feature/status bits; современные Apple TV UDP/PTP video/key profiles; MFi/FairPlay за пределами описанного FPLY; Handoff/SharePlay/cloud ownership; HomeKit accessory permissions/certification; Companion messages; полный DACP/DMAP remote path; все codecs/container profiles; PTP extensions/path delay/BMCA; MRP UI/group conflict/endpoint scope/error semantics; reverse HTTP/resource fetch lifecycle. Наличие поля в schema не устраняет UNKNOWN.

Также за пределами точного описания остаются весь legacy RSA Apple-Challenge/response и все варианты SDP key wrapping, полная HAP administration/pairings permission model и все codec-specific elementary payload layouts. Их нельзя восполнить именами endpoints или предположением, что один успешный receiver profile покрывает остальные.

Пассивный pcap после HAP verify не раскрывает plist без session keys. Wi-Fi key не снимает application AEAD; долговременный Ed25519 identity key не равен ephemeral X25519/SRP shared key. Отсутствие видимого ekey в зашифрованном трафике недоказательно; ключ может быть в связанной session. Обычный LAN host может не видеть unicast iPhone↔AppleTV.

## 12. Закреплённые первичные источники

Ссылки указывают на прочитанные исходники и подтверждают конкретные форматы выше. Core-ссылки закреплены на чистый commit 2b891199…; MRP descriptors происходят из pyatv. Appendix содержит срез wire registry, а не полный MRP.

[C-SESSION]: https://github.com/CorsairGuy/AirPlay/blob/2b8911990b3c05f8adf8de3ea29c2e491eb01141/bin/airplay/src/session.rs
[C-RTSP]: https://github.com/CorsairGuy/AirPlay/blob/2b8911990b3c05f8adf8de3ea29c2e491eb01141/crates/rtsp/src/lib.rs
[C-MEDIA]: https://github.com/CorsairGuy/AirPlay/blob/2b8911990b3c05f8adf8de3ea29c2e491eb01141/bin/airplay/src/media.rs
[C-DEVICE]: https://github.com/CorsairGuy/AirPlay/blob/2b8911990b3c05f8adf8de3ea29c2e491eb01141/crates/device/src/lib.rs
[C-HAP]: https://github.com/CorsairGuy/AirPlay/blob/2b8911990b3c05f8adf8de3ea29c2e491eb01141/crates/pair/src/hap.rs
[C-TLV]: https://github.com/CorsairGuy/AirPlay/blob/2b8911990b3c05f8adf8de3ea29c2e491eb01141/crates/pair/src/tlv8.rs
[C-SRP]: https://github.com/CorsairGuy/AirPlay/blob/2b8911990b3c05f8adf8de3ea29c2e491eb01141/crates/crypto/src/srp.rs
[C-KDF]: https://github.com/CorsairGuy/AirPlay/blob/2b8911990b3c05f8adf8de3ea29c2e491eb01141/crates/crypto/src/kdf.rs
[C-CHANNEL]: https://github.com/CorsairGuy/AirPlay/blob/2b8911990b3c05f8adf8de3ea29c2e491eb01141/crates/crypto/src/channel.rs
[C-RTP]: https://github.com/CorsairGuy/AirPlay/blob/2b8911990b3c05f8adf8de3ea29c2e491eb01141/crates/rtp/src/lib.rs
[C-NET]: https://github.com/CorsairGuy/AirPlay/blob/2b8911990b3c05f8adf8de3ea29c2e491eb01141/crates/audio_stream/src/net.rs
[C-NTP]: https://github.com/CorsairGuy/AirPlay/blob/2b8911990b3c05f8adf8de3ea29c2e491eb01141/services/timesync/src/ntp.rs
[C-PTP]: https://github.com/CorsairGuy/AirPlay/blob/2b8911990b3c05f8adf8de3ea29c2e491eb01141/crates/clock/src/ptp.rs
[C-REMOTE]: https://github.com/CorsairGuy/AirPlay/blob/2b8911990b3c05f8adf8de3ea29c2e491eb01141/bin/airplay/src/remote.rs
[C-MRPFRAME]: https://github.com/CorsairGuy/AirPlay/blob/2b8911990b3c05f8adf8de3ea29c2e491eb01141/crates/mrp/src/frame.rs
[C-MRPWIRE]: https://github.com/CorsairGuy/AirPlay/blob/2b8911990b3c05f8adf8de3ea29c2e491eb01141/crates/mrp/src/wire.rs
[C-SCHEMA]: https://github.com/CorsairGuy/AirPlay/blob/2b8911990b3c05f8adf8de3ea29c2e491eb01141/crates/mrp/src/schema.rs
[C-MESSAGES]: https://github.com/CorsairGuy/AirPlay/blob/2b8911990b3c05f8adf8de3ea29c2e491eb01141/crates/mrp/src/messages.rs
[C-MRPSERVICE]: https://github.com/CorsairGuy/AirPlay/blob/2b8911990b3c05f8adf8de3ea29c2e491eb01141/services/mediaremote/src/lib.rs
[C-MIRROR]: https://github.com/CorsairGuy/AirPlay/blob/2b8911990b3c05f8adf8de3ea29c2e491eb01141/crates/mirror/src/receiver.rs
[P-AP2]: https://github.com/postlund/pyatv/blob/b277a4c8222ecdcbaab8a24e3e713ca44765adb4/pyatv/protocols/airplay/ap2_session.py
[A-DISC]: https://github.com/openairplay/airplay-spec/blob/00063da0e7f16c20fbfb5d29331334da1a89590a/src/service_discovery.md
[A-FEATURES]: https://github.com/openairplay/airplay-spec/blob/00063da0e7f16c20fbfb5d29331334da1a89590a/src/features.md
[A-ANNOUNCE]: https://github.com/openairplay/airplay-spec/blob/00063da0e7f16c20fbfb5d29331334da1a89590a/src/audio/rtsp_requests/announce.md
[A-SETUP]: https://github.com/openairplay/airplay-spec/blob/00063da0e7f16c20fbfb5d29331334da1a89590a/src/audio/rtsp_requests/setup.md
[A-RTP]: https://github.com/openairplay/airplay-spec/blob/00063da0e7f16c20fbfb5d29331334da1a89590a/src/audio/rtp_streams.md
[A-META]: https://github.com/openairplay/airplay-spec/blob/00063da0e7f16c20fbfb5d29331334da1a89590a/src/audio/metadata.md
[A-VOLUME]: https://github.com/openairplay/airplay-spec/blob/00063da0e7f16c20fbfb5d29331334da1a89590a/src/audio/volume_control.md
[A-VIDEO]: https://github.com/openairplay/airplay-spec/blob/00063da0e7f16c20fbfb5d29331334da1a89590a/src/video/http_requests.md
[U-DNSSD]: https://github.com/FDH2/UxPlay/blob/534e2cc0e558d6c94bcc66964f8fc13ad73a6cdc/lib/dnssdint.h
[U-HANDLERS]: https://github.com/FDH2/UxPlay/blob/534e2cc0e558d6c94bcc66964f8fc13ad73a6cdc/lib/raop_handlers.h
[U-FPLY]: https://github.com/FDH2/UxPlay/blob/534e2cc0e558d6c94bcc66964f8fc13ad73a6cdc/lib/fairplay_playfair.c
[U-CRYPTO]: https://github.com/FDH2/UxPlay/blob/534e2cc0e558d6c94bcc66964f8fc13ad73a6cdc/lib/crypto.c
[U-MIRROR]: https://github.com/FDH2/UxPlay/blob/534e2cc0e558d6c94bcc66964f8fc13ad73a6cdc/lib/mirror_buffer.c
[S-AP2]: https://github.com/mikebrady/shairport-sync/blob/0a34fbda83724cc06266aadcf7c88acc8fef4e7d/AIRPLAY2.md
[N-README]: https://github.com/mikebrady/nqptp/blob/ab8fb67483411640fd7f1e193889482f2421c37c/README.md
[R-STATUS]: https://github.com/metaneutrons/shairplay-rust/blob/2fb72b30b658b33f106165a5b6fbd941919bdbe5/AP2-STATUS.md





## Appendix A. Все 609 MRP field mappings проверенного core snapshot

Автоматически получено из field constants и всех wire_type match arms [C-SCHEMA], включая multiline arms в braces. SHA256 локального schema.rs: `8d4bb73b44dd3135a180486a726a15131dc849f5b616cbbcbcff4e67eb66b543`. Поля: message, field name, numeric tag, wire type, nested message. Wire0 не определяет signedness/enum/bool; wire2 не различает string/data без полного descriptor. Повторяемость и required/optional здесь автоматически не выводятся. Это полная таблица данного среза, не полная спецификация MRP.

| Message | Field | Tag | Wire | Nested message |
|---|---|---|---|---|
| AVEndpointDescriptor | name | 1 | 2 | — |
| AVEndpointDescriptor | uniqueIdentifier | 2 | 2 | — |
| AVEndpointDescriptor | isLocalEndpoint | 5 | 0 | — |
| AVEndpointDescriptor | instanceIdentifier | 6 | 2 | — |
| AVEndpointDescriptor | isProxyGroupPlayer | 7 | 0 | — |
| AVEndpointDescriptor | connectionType | 8 | 0 | — |
| AVEndpointDescriptor | canModifyGroupMembership | 9 | 0 | — |
| AVOutputDeviceDescriptor | name | 1 | 2 | — |
| AVOutputDeviceDescriptor | uniqueIdentifier | 2 | 2 | — |
| AVOutputDeviceDescriptor | groupID | 3 | 2 | — |
| AVOutputDeviceDescriptor | modelID | 4 | 2 | — |
| AVOutputDeviceDescriptor | macAddress | 5 | 2 | — |
| AVOutputDeviceDescriptor | canAccessRemoteAssets | 6 | 0 | — |
| AVOutputDeviceDescriptor | isRemoteControllable | 7 | 0 | — |
| AVOutputDeviceDescriptor | isGroupLeader | 8 | 0 | — |
| AVOutputDeviceDescriptor | isGroupable | 9 | 0 | — |
| AVOutputDeviceDescriptor | deviceType | 10 | 0 | — |
| AVOutputDeviceDescriptor | deviceSubType | 11 | 0 | — |
| AVOutputDeviceDescriptor | modelSpecificInfoData | 12 | 2 | — |
| AVOutputDeviceDescriptor | batteryLevel | 13 | 5 | — |
| AVOutputDeviceDescriptor | isLocalDevice | 14 | 0 | — |
| AVOutputDeviceDescriptor | supportsExternalScreen | 15 | 0 | — |
| AVOutputDeviceDescriptor | requiresAuthorization | 16 | 0 | — |
| AVOutputDeviceDescriptor | shouldForceRemoteControlabillity | 17 | 0 | — |
| AVOutputDeviceDescriptor | sourceInfo | 18 | 2 | AVOutputDeviceSourceInfo |
| AVOutputDeviceDescriptor | isDeviceGroupable | 19 | 0 | — |
| AVOutputDeviceDescriptor | canRelayCommunicationChannel | 20 | 0 | — |
| AVOutputDeviceDescriptor | logicalDeviceID | 21 | 2 | — |
| AVOutputDeviceDescriptor | isProxyGroupPlayer | 22 | 0 | — |
| AVOutputDeviceDescriptor | firmwareVersion | 23 | 2 | — |
| AVOutputDeviceDescriptor | volume | 24 | 5 | — |
| AVOutputDeviceDescriptor | isVolumeControlAvailable | 25 | 0 | — |
| AVOutputDeviceDescriptor | canAccessAppleMusic | 26 | 0 | — |
| AVOutputDeviceDescriptor | canAccessiCloudMusicLibrary | 27 | 0 | — |
| AVOutputDeviceDescriptor | groupContainsGroupLeader | 28 | 0 | — |
| AVOutputDeviceDescriptor | supportsBufferedAirPlay | 29 | 0 | — |
| AVOutputDeviceDescriptor | canPlayEncryptedProgressiveDownloadAssets | 30 | 0 | — |
| AVOutputDeviceDescriptor | canFetchMediaDataFromSender | 31 | 0 | — |
| AVOutputDeviceDescriptor | resentsOptimizedUserInterfaceWhenPlayingFetchedAudioOnlyAssets | 32 | 0 | — |
| AVOutputDeviceDescriptor | isAirPlayReceiverSessionActive | 33 | 0 | — |
| AVOutputDeviceDescriptor | parentGroupIdentifier | 34 | 2 | — |
| AVOutputDeviceDescriptor | parentGroupContainsDiscoverableLeader | 35 | 0 | — |
| AVOutputDeviceDescriptor | isAddedToHomeKit | 36 | 0 | — |
| AVOutputDeviceDescriptor | volumeCapabilities | 37 | 0 | — |
| AVOutputDeviceDescriptor | bluetoothID | 38 | 2 | — |
| AVOutputDeviceDescriptor | supportsHAP | 39 | 0 | — |
| AVOutputDeviceDescriptor | usingJSONProtocol | 40 | 0 | — |
| AVOutputDeviceDescriptor | clusterCompositions | 41 | 2 | AVOutputDeviceDescriptor |
| AVOutputDeviceDescriptor | clusterType | 42 | 0 | — |
| AVOutputDeviceDescriptor | primaryUID | 43 | 2 | — |
| AVOutputDeviceDescriptor | configuredClusterSize | 44 | 0 | — |
| AVOutputDeviceDescriptor | supportsRapportRemoteControlTransport | 45 | 0 | — |
| AVOutputDeviceDescriptor | currentBluetoothListeningMode | 46 | 2 | — |
| AVOutputDeviceDescriptor | availableBluetoothListeningModes | 47 | 2 | — |
| AVOutputDeviceDescriptor | supportsMultiplayer | 48 | 0 | — |
| AVOutputDeviceDescriptor | producesLowFidelityAudio | 49 | 0 | — |
| AVOutputDeviceDescriptor | hostDeviceClass | 50 | 0 | — |
| AVOutputDeviceDescriptor | airPlayGroupID | 51 | 2 | — |
| AVOutputDeviceDescriptor | supportsSharePlayHandoff | 52 | 0 | — |
| AVOutputDeviceDescriptor | distance | 53 | 5 | — |
| AVOutputDeviceDescriptor | discoveredOnSameInfra | 54 | 0 | — |
| AVOutputDeviceDescriptor | activatedClusterMembers | 55 | 2 | AVOutputDeviceDescriptor |
| AVOutputDeviceDescriptor | pickable | 56 | 0 | — |
| AVOutputDeviceDescriptor | transportType | 57 | 0 | — |
| AVOutputDeviceDescriptor | clusterID | 58 | 2 | — |
| AVOutputDeviceDescriptor | isClusterLeader | 59 | 0 | — |
| AVOutputDeviceDescriptor | isAppleAccessory | 60 | 0 | — |
| AVOutputDeviceDescriptor | parentUniqueIdentifier | 61 | 2 | — |
| AVOutputDeviceDescriptor | roomID | 62 | 2 | — |
| AVOutputDeviceDescriptor | roomName | 63 | 2 | — |
| AVOutputDeviceDescriptor | allClusterMembers | 64 | 2 | AVOutputDeviceDescriptor |
| AVOutputDeviceDescriptor | engageOnClusterActivate | 67 | 0 | — |
| AVOutputDeviceSourceInfo | routingContextUID | 1 | 2 | — |
| AVOutputDeviceSourceInfo | multipleBuiltInDevices | 2 | 0 | — |
| AudioBuffer | formatSettings | 1 | 2 | AudioFormatSettings |
| AudioBuffer | packetCapacity | 2 | 0 | — |
| AudioBuffer | maximumPacketSize | 3 | 0 | — |
| AudioBuffer | packetCount | 4 | 0 | — |
| AudioBuffer | contents | 5 | 2 | — |
| AudioBuffer | packetDescriptions | 6 | 2 | AudioStreamPacketDescription |
| AudioDataBlock | buffer | 1 | 2 | AudioBuffer |
| AudioDataBlock | time | 2 | 2 | AudioTime |
| AudioDataBlock | gain | 3 | 1 | — |
| AudioFadeMessage | playerPath | 1 | 2 | PlayerPath |
| AudioFadeMessage | fadeType | 2 | 0 | — |
| AudioFadeResponseMessage | fadeDuration | 1 | 0 | — |
| AudioFormat | tier | 1 | 0 | — |
| AudioFormat | bitrate | 2 | 0 | — |
| AudioFormat | sampleRate | 3 | 0 | — |
| AudioFormat | bitDepth | 4 | 0 | — |
| AudioFormat | codec | 5 | 0 | — |
| AudioFormat | spatialized | 6 | 0 | — |
| AudioFormat | multiChannel | 7 | 0 | — |
| AudioFormat | channelLayout | 8 | 0 | — |
| AudioFormat | audioChannelLayoutDescription | 9 | 2 | — |
| AudioFormat | groupID | 10 | 2 | — |
| AudioFormat | stableVariantID | 11 | 2 | — |
| AudioFormatSettings | formatSettingsPlistData | 1 | 2 | — |
| AudioRoute | type | 1 | 2 | AudioRouteType |
| AudioRoute | name | 2 | 2 | — |
| AudioRoute | supportsSpatialization | 3 | 0 | — |
| AudioStreamPacketDescription | startOffset | 1 | 0 | — |
| AudioStreamPacketDescription | variableFramesInPacket | 2 | 0 | — |
| AudioStreamPacketDescription | dataByteSize | 3 | 0 | — |
| AudioTime | timestamp | 1 | 1 | — |
| AudioTime | sampleRate | 2 | 1 | — |
| ClientUpdatesConfigMessage | artworkUpdates | 1 | 0 | — |
| ClientUpdatesConfigMessage | nowPlayingUpdates | 2 | 0 | — |
| ClientUpdatesConfigMessage | volumeUpdates | 3 | 0 | — |
| ClientUpdatesConfigMessage | keyboardUpdates | 4 | 0 | — |
| ClientUpdatesConfigMessage | outputDeviceUpdates | 5 | 0 | — |
| CommandInfo | command | 1 | 0 | — |
| CommandInfo | enabled | 2 | 0 | — |
| CommandInfo | active | 3 | 0 | — |
| CommandInfo | preferredIntervals | 4 | 1 | — |
| CommandInfo | localizedTitle | 5 | 2 | — |
| CommandInfo | minimumRating | 6 | 5 | — |
| CommandInfo | maximumRating | 7 | 5 | — |
| CommandInfo | supportedRates | 8 | 5 | — |
| CommandInfo | localizedShortTitle | 9 | 2 | — |
| CommandInfo | repeatMode | 10 | 0 | — |
| CommandInfo | shuffleMode | 11 | 0 | — |
| CommandInfo | presentationStyle | 12 | 0 | — |
| CommandInfo | skipInterval | 13 | 0 | — |
| CommandInfo | numAvailableSkips | 14 | 0 | — |
| CommandInfo | skipFrequency | 15 | 0 | — |
| CommandInfo | canScrub | 16 | 0 | — |
| CommandInfo | supportedPlaybackQueueTypes | 17 | 0 | — |
| CommandInfo | supportedCustomQueueIdentifiers | 18 | 2 | — |
| CommandInfo | supportedInsertionPositions | 19 | 0 | — |
| CommandInfo | supportsSharedQueue | 20 | 0 | — |
| CommandInfo | upNextItemCount | 21 | 0 | — |
| CommandInfo | preferredPlaybackRate | 22 | 5 | — |
| CommandInfo | supportedPlaybackSessionTypes | 23 | 2 | — |
| CommandInfo | currentPlaybackSessionTypes | 24 | 2 | — |
| CommandInfo | playbackSessionIdentifier | 25 | 2 | — |
| CommandInfo | currentQueueEndAction | 26 | 0 | — |
| CommandInfo | supportedEndQueueActions | 27 | 0 | — |
| CommandInfo | disableReason | 28 | 0 | — |
| CommandInfo | supportedPlaybackSessionIdentifiers | 29 | 2 | PreloadedPlaybackSessionInfo |
| CommandOptions | sourceId | 2 | 2 | — |
| CommandOptions | mediaType | 3 | 2 | — |
| CommandOptions | externalPlayerCommand | 4 | 0 | — |
| CommandOptions | skipInterval | 5 | 5 | — |
| CommandOptions | playbackRate | 6 | 5 | — |
| CommandOptions | rating | 7 | 5 | — |
| CommandOptions | negative | 8 | 0 | — |
| CommandOptions | playbackPosition | 9 | 1 | — |
| CommandOptions | repeatMode | 10 | 0 | — |
| CommandOptions | shuffleMode | 11 | 0 | — |
| CommandOptions | trackID | 12 | 0 | — |
| CommandOptions | radioStationID | 13 | 0 | — |
| CommandOptions | radioStationHash | 14 | 2 | — |
| CommandOptions | systemAppPlaybackQueueData | 15 | 2 | — |
| CommandOptions | destinationAppDisplayID | 16 | 2 | — |
| CommandOptions | sendOptions | 17 | 0 | — |
| CommandOptions | requestDefermentToPlaybackQueuePosition | 18 | 0 | — |
| CommandOptions | contextID | 19 | 2 | — |
| CommandOptions | shouldOverrideManuallyCuratedQueue | 20 | 0 | — |
| CommandOptions | stationURL | 21 | 2 | — |
| CommandOptions | shouldBeginRadioPlayback | 22 | 0 | — |
| CommandOptions | playbackQueueInsertionPosition | 23 | 0 | — |
| CommandOptions | contentItemID | 24 | 2 | — |
| CommandOptions | playbackQueueOffset | 25 | 0 | — |
| CommandOptions | playbackQueueDestinationOffset | 26 | 0 | — |
| CommandOptions | languageOption | 27 | 2 | — |
| CommandOptions | playbackQueueContext | 28 | 2 | — |
| CommandOptions | insertAfterContentItemID | 29 | 2 | — |
| CommandOptions | nowPlayingContentItemID | 30 | 2 | — |
| CommandOptions | replaceIntent | 31 | 0 | — |
| ConfigureConnectionMessage | groupID | 1 | 2 | — |
| ContentItem | identifier | 1 | 2 | — |
| ContentItem | metadata | 2 | 2 | ContentItemMetadata |
| ContentItem | artworkData | 3 | 2 | — |
| ContentItem | info | 4 | 2 | — |
| ContentItem | availableLanguageOptions | 5 | 2 | LanguageOptionGroup |
| ContentItem | currentLanguageOptions | 6 | 2 | LanguageOption |
| ContentItem | parentIdentifier | 9 | 2 | — |
| ContentItem | ancestorIdentifier | 10 | 2 | — |
| ContentItem | queueIdentifier | 11 | 2 | — |
| ContentItem | requestIdentifier | 12 | 2 | — |
| ContentItem | artworkDataWidth | 13 | 0 | — |
| ContentItem | artworkDataHeight | 14 | 0 | — |
| ContentItemMetadata | title | 1 | 2 | — |
| ContentItemMetadata | subtitle | 2 | 2 | — |
| ContentItemMetadata | isContainer | 3 | 0 | — |
| ContentItemMetadata | isPlayable | 4 | 0 | — |
| ContentItemMetadata | playbackProgress | 5 | 5 | — |
| ContentItemMetadata | albumName | 6 | 2 | — |
| ContentItemMetadata | trackArtistName | 7 | 2 | — |
| ContentItemMetadata | albumArtistName | 8 | 2 | — |
| ContentItemMetadata | directorName | 9 | 2 | — |
| ContentItemMetadata | seasonNumber | 10 | 0 | — |
| ContentItemMetadata | episodeNumber | 11 | 0 | — |
| ContentItemMetadata | releaseDate | 12 | 1 | — |
| ContentItemMetadata | playCount | 13 | 0 | — |
| ContentItemMetadata | duration | 14 | 1 | — |
| ContentItemMetadata | localizedContentRating | 15 | 2 | — |
| ContentItemMetadata | isExplicitItem | 16 | 0 | — |
| ContentItemMetadata | playlistType | 17 | 0 | — |
| ContentItemMetadata | radioStationType | 18 | 0 | — |
| ContentItemMetadata | artworkAvailable | 19 | 0 | — |
| ContentItemMetadata | infoAvailable | 21 | 0 | — |
| ContentItemMetadata | languageOptionsAvailable | 22 | 0 | — |
| ContentItemMetadata | numberOfSections | 23 | 0 | — |
| ContentItemMetadata | lyricsAvailable | 24 | 0 | — |
| ContentItemMetadata | editingStyleFlags | 25 | 0 | — |
| ContentItemMetadata | isStreamingContent | 26 | 0 | — |
| ContentItemMetadata | isCurrentlyPlaying | 27 | 0 | — |
| ContentItemMetadata | collectionIdentifier | 28 | 2 | — |
| ContentItemMetadata | profileIdentifier | 29 | 2 | — |
| ContentItemMetadata | startTime | 30 | 1 | — |
| ContentItemMetadata | artworkMIMEType | 31 | 2 | — |
| ContentItemMetadata | assetURLString | 32 | 2 | — |
| ContentItemMetadata | composer | 33 | 2 | — |
| ContentItemMetadata | discNumber | 34 | 0 | — |
| ContentItemMetadata | elapsedTime | 35 | 1 | — |
| ContentItemMetadata | genre | 36 | 2 | — |
| ContentItemMetadata | isAlwaysLive | 37 | 0 | — |
| ContentItemMetadata | playbackRate | 39 | 5 | — |
| ContentItemMetadata | chapterCount | 40 | 0 | — |
| ContentItemMetadata | totalDiscCount | 41 | 0 | — |
| ContentItemMetadata | totalTrackCount | 42 | 0 | — |
| ContentItemMetadata | trackNumber | 43 | 0 | — |
| ContentItemMetadata | contentIdentifier | 44 | 2 | — |
| ContentItemMetadata | isSharable | 46 | 0 | — |
| ContentItemMetadata | isLiked | 48 | 0 | — |
| ContentItemMetadata | isInWishList | 49 | 0 | — |
| ContentItemMetadata | radioStationIdentifier | 50 | 0 | — |
| ContentItemMetadata | radioStationName | 52 | 2 | — |
| ContentItemMetadata | radioStationString | 53 | 2 | — |
| ContentItemMetadata | iTunesStoreIdentifier | 54 | 0 | — |
| ContentItemMetadata | iTunesStoreSubscriptionIdentifier | 55 | 0 | — |
| ContentItemMetadata | iTunesStoreArtistIdentifier | 56 | 0 | — |
| ContentItemMetadata | iTunesStoreAlbumIdentifier | 57 | 0 | — |
| ContentItemMetadata | purchaseInfoData | 58 | 2 | — |
| ContentItemMetadata | defaultPlaybackRate | 59 | 5 | — |
| ContentItemMetadata | downloadState | 60 | 0 | — |
| ContentItemMetadata | downloadProgress | 61 | 5 | — |
| ContentItemMetadata | appMetricsData | 62 | 2 | — |
| ContentItemMetadata | seriesName | 63 | 2 | — |
| ContentItemMetadata | mediaType | 64 | 0 | — |
| ContentItemMetadata | mediaSubType | 65 | 0 | — |
| ContentItemMetadata | nowPlayingInfoData | 67 | 2 | — |
| ContentItemMetadata | userInfoData | 68 | 2 | — |
| ContentItemMetadata | isSteerable | 69 | 0 | — |
| ContentItemMetadata | artworkURL | 70 | 2 | — |
| ContentItemMetadata | lyricsURL | 71 | 2 | — |
| ContentItemMetadata | deviceSpecificUserInfoData | 72 | 2 | — |
| ContentItemMetadata | collectionInfoData | 73 | 2 | — |
| ContentItemMetadata | elapsedTimeTimestamp | 74 | 1 | — |
| ContentItemMetadata | inferredTimestamp | 75 | 1 | — |
| ContentItemMetadata | serviceIdentifier | 76 | 2 | — |
| ContentItemMetadata | artworkDataWidth | 77 | 0 | — |
| ContentItemMetadata | artworkDataHeight | 78 | 0 | — |
| ContentItemMetadata | currentPlaybackDateData | 79 | 2 | — |
| ContentItemMetadata | artworkIdentifier | 80 | 2 | — |
| ContentItemMetadata | isLoading | 81 | 0 | — |
| ContentItemMetadata | artworkURLTemplatesData | 82 | 2 | — |
| ContentItemMetadata | legacyUniqueIdentifier | 83 | 0 | — |
| ContentItemMetadata | episodeType | 84 | 0 | — |
| ContentItemMetadata | artworkFileURL | 85 | 2 | — |
| ContentItemMetadata | brandIdentifier | 86 | 2 | — |
| ContentItemMetadata | localizedDurationString | 87 | 2 | — |
| ContentItemMetadata | albumYear | 88 | 2 | — |
| ContentItemMetadata | songTraits | 89 | 0 | — |
| ContentItemMetadata | albumTraits | 90 | 0 | — |
| ContentItemMetadata | playlistTraits | 91 | 0 | — |
| ContentItemMetadata | preferredFormat | 92 | 2 | AudioFormat |
| ContentItemMetadata | activeFormat | 93 | 2 | AudioFormat |
| ContentItemMetadata | activeFormatJustification | 94 | 0 | — |
| ContentItemMetadata | formatTierPreference | 95 | 0 | — |
| ContentItemMetadata | audioRoute | 96 | 2 | AudioRoute |
| ContentItemMetadata | alternativeFormats | 97 | 2 | AudioFormat |
| CryptoPairingMessage | pairingData | 1 | 2 | — |
| CryptoPairingMessage | status | 2 | 0 | — |
| CryptoPairingMessage | isRetrying | 3 | 0 | — |
| CryptoPairingMessage | isUsingSystemPairing | 4 | 0 | — |
| CryptoPairingMessage | state | 5 | 0 | — |
| DeviceInfoMessage | uniqueIdentifier | 1 | 2 | — |
| DeviceInfoMessage | name | 2 | 2 | — |
| DeviceInfoMessage | localizedModelName | 3 | 2 | — |
| DeviceInfoMessage | systemBuildVersion | 4 | 2 | — |
| DeviceInfoMessage | applicationBundleIdentifier | 5 | 2 | — |
| DeviceInfoMessage | applicationBundleVersion | 6 | 2 | — |
| DeviceInfoMessage | protocolVersion | 7 | 0 | — |
| DeviceInfoMessage | lastSupportedMessageType | 8 | 0 | — |
| DeviceInfoMessage | supportsSystemPairing | 9 | 0 | — |
| DeviceInfoMessage | allowsPairing | 10 | 0 | — |
| DeviceInfoMessage | connected | 11 | 0 | — |
| DeviceInfoMessage | systemMediaApplication | 12 | 2 | — |
| DeviceInfoMessage | supportsACL | 13 | 0 | — |
| DeviceInfoMessage | supportsSharedQueue | 14 | 0 | — |
| DeviceInfoMessage | supportsExtendedMotion | 15 | 0 | — |
| DeviceInfoMessage | bluetoothAddress | 16 | 2 | — |
| DeviceInfoMessage | sharedQueueVersion | 17 | 0 | — |
| DeviceInfoMessage | deviceUID | 19 | 2 | — |
| DeviceInfoMessage | managedConfigDeviceID | 20 | 2 | — |
| DeviceInfoMessage | deviceClass | 21 | 0 | — |
| DeviceInfoMessage | logicalDeviceCount | 22 | 0 | — |
| DeviceInfoMessage | tightlySyncedGroup | 23 | 0 | — |
| DeviceInfoMessage | isProxyGroupPlayer | 24 | 0 | — |
| DeviceInfoMessage | tightSyncUID | 25 | 2 | — |
| DeviceInfoMessage | groupUID | 26 | 2 | — |
| DeviceInfoMessage | groupName | 27 | 2 | — |
| DeviceInfoMessage | groupedDevices | 28 | 2 | DeviceInfoMessage |
| DeviceInfoMessage | isGroupLeader | 29 | 0 | — |
| DeviceInfoMessage | isAirplayActive | 30 | 0 | — |
| DeviceInfoMessage | systemPodcastApplication | 31 | 2 | — |
| DeviceInfoMessage | senderDefaultGroupUID | 32 | 2 | — |
| DeviceInfoMessage | airplayReceivers | 33 | 2 | — |
| DeviceInfoMessage | linkAgent | 34 | 2 | — |
| DeviceInfoMessage | clusterID | 35 | 2 | — |
| DeviceInfoMessage | clusterLeaderID | 36 | 2 | — |
| DeviceInfoMessage | clusterType | 37 | 0 | — |
| DeviceInfoMessage | isClusterAware | 38 | 0 | — |
| DeviceInfoMessage | modelID | 39 | 2 | — |
| DeviceInfoMessage | supportsMultiplayer | 40 | 0 | — |
| DeviceInfoMessage | routingContextID | 41 | 2 | — |
| DeviceInfoMessage | airPlayGroupID | 42 | 2 | — |
| DeviceInfoMessage | systemBooksApplication | 43 | 2 | — |
| DeviceInfoMessage | clusteredDevices | 44 | 2 | DeviceInfoMessage |
| DeviceInfoMessage | parentGroupContainsDiscoverableGroupLeader | 45 | 0 | — |
| DeviceInfoMessage | groupContainsDiscoverableGroupLeader | 46 | 0 | — |
| DeviceInfoMessage | lastKnownClusterType | 47 | 0 | — |
| DeviceInfoMessage | supportsOutputContextSync | 49 | 0 | — |
| DeviceInfoMessage | computerName | 50 | 2 | — |
| DeviceInfoMessage | configuredClusterSize | 51 | 0 | — |
| DeviceInfoMessage | preferredEncoding | 52 | 0 | — |
| GenericMessage | key | 1 | 2 | — |
| GenericMessage | value | 2 | 2 | — |
| GetVolumeMessage | outputDeviceUID | 1 | 2 | — |
| GetVolumeResultMessage | volume | 1 | 5 | — |
| KeyboardMessage | state | 1 | 0 | — |
| KeyboardMessage | attributes | 3 | 2 | TextEditingAttributes |
| KeyboardMessage | encryptedTextCyphertext | 4 | 2 | — |
| LanguageOption | type | 1 | 0 | — |
| LanguageOption | languageTag | 2 | 2 | — |
| LanguageOption | characteristics | 3 | 2 | — |
| LanguageOption | displayName | 4 | 2 | — |
| LanguageOption | identifier | 5 | 2 | — |
| LanguageOptionGroup | allowEmptySelection | 1 | 0 | — |
| LanguageOptionGroup | defaultLanguageOption | 2 | 2 | LanguageOption |
| LanguageOptionGroup | languageOptions | 3 | 2 | LanguageOption |
| ModifyOutputContextRequestMessage | type | 1 | 0 | — |
| ModifyOutputContextRequestMessage | addingDevices | 2 | 2 | — |
| ModifyOutputContextRequestMessage | removingDevices | 3 | 2 | — |
| ModifyOutputContextRequestMessage | settingDevices | 4 | 2 | — |
| ModifyOutputContextRequestMessage | clusterAwareAddingDevices | 5 | 2 | — |
| ModifyOutputContextRequestMessage | clusterAwareRemovingDevices | 6 | 2 | — |
| ModifyOutputContextRequestMessage | clusterAwareSettingDevices | 7 | 2 | — |
| NotificationMessage | notification | 1 | 2 | — |
| NotificationMessage | userInfo | 2 | 2 | — |
| NowPlayingClient | processIdentifier | 1 | 0 | — |
| NowPlayingClient | bundleIdentifier | 2 | 2 | — |
| NowPlayingClient | parentApplicationBundleIdentifier | 3 | 2 | — |
| NowPlayingClient | processUserIdentifier | 4 | 0 | — |
| NowPlayingClient | nowPlayingVisibility | 5 | 0 | — |
| NowPlayingClient | displayName | 7 | 2 | — |
| NowPlayingClient | bundleIdentifierHierarchys | 8 | 2 | — |
| NowPlayingInfo | album | 1 | 2 | — |
| NowPlayingInfo | artist | 2 | 2 | — |
| NowPlayingInfo | duration | 3 | 1 | — |
| NowPlayingInfo | elapsedTime | 4 | 1 | — |
| NowPlayingInfo | playbackRate | 5 | 5 | — |
| NowPlayingInfo | repeatMode | 6 | 0 | — |
| NowPlayingInfo | shuffleMode | 7 | 0 | — |
| NowPlayingInfo | timestamp | 8 | 1 | — |
| NowPlayingInfo | title | 9 | 2 | — |
| NowPlayingInfo | uniqueIdentifier | 10 | 0 | — |
| NowPlayingInfo | isExplicitTrack | 11 | 0 | — |
| NowPlayingInfo | isMusicApp | 12 | 0 | — |
| NowPlayingInfo | radioStationIdentifier | 13 | 0 | — |
| NowPlayingInfo | radioStationHash | 14 | 2 | — |
| NowPlayingInfo | radioStationName | 15 | 2 | — |
| NowPlayingInfo | artworkDataDigest | 16 | 2 | — |
| NowPlayingInfo | isAlwaysLive | 17 | 0 | — |
| NowPlayingInfo | isAdvertisement | 18 | 0 | — |
| NowPlayingPlayer | identifier | 1 | 2 | — |
| NowPlayingPlayer | displayName | 2 | 2 | — |
| NowPlayingPlayer | isDefaultPlayer | 3 | 0 | — |
| NowPlayingPlayer | audioSessionType | 4 | 0 | — |
| NowPlayingPlayer | mxSessionIDs | 5 | 0 | — |
| NowPlayingPlayer | audioSessionID | 6 | 0 | — |
| NowPlayingPlayer | iconURL | 7 | 2 | — |
| Origin | type | 1 | 0 | — |
| Origin | displayName | 2 | 2 | — |
| Origin | identifier | 3 | 0 | — |
| Origin | deviceInfo | 4 | 2 | DeviceInfoMessage |
| Origin | isLocallyHosted | 5 | 0 | — |
| OriginClientPropertiesMessage | lastPlayingTimestamp | 1 | 1 | — |
| PlaybackQueue | location | 1 | 0 | — |
| PlaybackQueue | contentItems | 2 | 2 | ContentItem |
| PlaybackQueue | context | 3 | 2 | PlaybackQueueContext |
| PlaybackQueue | requestId | 4 | 2 | — |
| PlaybackQueue | resolvedPlayerPath | 5 | 2 | PlayerPath |
| PlaybackQueue | sendingPlaybackQueueTransaction | 6 | 0 | — |
| PlaybackQueue | queueIdentifier | 7 | 2 | — |
| PlaybackQueueCapabilities | requestByRange | 1 | 0 | — |
| PlaybackQueueCapabilities | requestByIdentifiers | 2 | 0 | — |
| PlaybackQueueCapabilities | requestByRequest | 3 | 0 | — |
| PlaybackQueueContext | revision | 1 | 2 | — |
| PlaybackQueueRequestMessage | location | 1 | 0 | — |
| PlaybackQueueRequestMessage | length | 2 | 0 | — |
| PlaybackQueueRequestMessage | includeMetadata | 3 | 0 | — |
| PlaybackQueueRequestMessage | artworkWidth | 4 | 1 | — |
| PlaybackQueueRequestMessage | artworkHeight | 5 | 1 | — |
| PlaybackQueueRequestMessage | includeLyrics | 6 | 0 | — |
| PlaybackQueueRequestMessage | includeSections | 7 | 0 | — |
| PlaybackQueueRequestMessage | includeInfo | 8 | 0 | — |
| PlaybackQueueRequestMessage | includeLanguageOptions | 9 | 0 | — |
| PlaybackQueueRequestMessage | context | 10 | 2 | PlaybackQueueContext |
| PlaybackQueueRequestMessage | requestID | 11 | 2 | — |
| PlaybackQueueRequestMessage | contentItemIdentifiers | 12 | 2 | — |
| PlaybackQueueRequestMessage | returnContentItemAssetsInUserCompletion | 13 | 0 | — |
| PlaybackQueueRequestMessage | playerPath | 14 | 2 | PlayerPath |
| PlaybackQueueRequestMessage | cachingPolicy | 15 | 0 | — |
| PlaybackQueueRequestMessage | label | 16 | 2 | — |
| PlaybackQueueRequestMessage | isLegacyNowPlayingInfoRequest | 17 | 0 | — |
| PlayerClientPropertiesMessage | playerPath | 1 | 2 | PlayerPath |
| PlayerClientPropertiesMessage | lastPlayingTimestamp | 2 | 1 | — |
| PlayerPath | origin | 1 | 2 | Origin |
| PlayerPath | client | 2 | 2 | NowPlayingClient |
| PlayerPath | player | 3 | 2 | NowPlayingPlayer |
| PreloadedPlaybackSessionInfo | playbackSessionIdentifier | 1 | 2 | — |
| PreloadedPlaybackSessionInfo | playbackSessionRevision | 2 | 2 | — |
| PreloadedPlaybackSessionInfo | playbackSessionPriority | 3 | 0 | — |
| ProtocolMessage | audioFadeMessage | 88 | 2 | AudioFadeMessage |
| ProtocolMessage | audioFadeResponseMessage | 89 | 2 | AudioFadeResponseMessage |
| ProtocolMessage | clientUpdatesConfigMessage | 21 | 2 | ClientUpdatesConfigMessage |
| ProtocolMessage | configureConnectionMessage | 94 | 2 | ConfigureConnectionMessage |
| ProtocolMessage | cryptoPairingMessage | 39 | 2 | CryptoPairingMessage |
| ProtocolMessage | deviceInfoMessage | 20 | 2 | DeviceInfoMessage |
| ProtocolMessage | genericMessage | 46 | 2 | GenericMessage |
| ProtocolMessage | getKeyboardSessionMessage | 29 | 2 | — |
| ProtocolMessage | getRemoteTextInputSessionMessage | 72 | 2 | GetRemoteTextInputSessionMessage |
| ProtocolMessage | getVolumeMessage | 53 | 2 | GetVolumeMessage |
| ProtocolMessage | getVolumeResultMessage | 54 | 2 | GetVolumeResultMessage |
| ProtocolMessage | keyboardMessage | 28 | 2 | KeyboardMessage |
| ProtocolMessage | modifyOutputContextRequestMessage | 52 | 2 | ModifyOutputContextRequestMessage |
| ProtocolMessage | notificationMessage | 16 | 2 | NotificationMessage |
| ProtocolMessage | originClientPropertiesMessage | 87 | 2 | OriginClientPropertiesMessage |
| ProtocolMessage | playbackQueueRequestMessage | 37 | 2 | PlaybackQueueRequestMessage |
| ProtocolMessage | playerClientPropertiesMessage | 86 | 2 | PlayerClientPropertiesMessage |
| ProtocolMessage | type | 1 | 0 | — |
| ProtocolMessage | identifier | 2 | 2 | — |
| ProtocolMessage | authenticationToken | 3 | 2 | — |
| ProtocolMessage | errorCode | 4 | 0 | — |
| ProtocolMessage | timestamp | 5 | 0 | — |
| ProtocolMessage | errorDescription | 78 | 2 | — |
| ProtocolMessage | uniqueIdentifier | 85 | 2 | — |
| ProtocolMessage | registerForGameControllerEventsMessage | 27 | 2 | RegisterForGameControllerEventsMessage |
| ProtocolMessage | registerHIDDeviceMessage | 11 | 2 | RegisterHIDDeviceMessage |
| ProtocolMessage | registerHIDDeviceResultMessage | 12 | 2 | RegisterHIDDeviceResultMessage |
| ProtocolMessage | registerVoiceInputDeviceMessage | 33 | 2 | RegisterVoiceInputDeviceMessage |
| ProtocolMessage | registerVoiceInputDeviceResponseMessage | 34 | 2 | RegisterVoiceInputDeviceResponseMessage |
| ProtocolMessage | remoteTextInputMessage | 71 | 2 | RemoteTextInputMessage |
| ProtocolMessage | removeClientMessage | 57 | 2 | RemoveClientMessage |
| ProtocolMessage | removeEndpointsMessage | 84 | 2 | RemoveEndpointsMessage |
| ProtocolMessage | removeOutputDevicesMessage | 70 | 2 | RemoveOutputDevicesMessage |
| ProtocolMessage | removePlayerMessage | 58 | 2 | RemovePlayerMessage |
| ProtocolMessage | sendButtonEventMessage | 43 | 2 | SendButtonEventMessage |
| ProtocolMessage | sendCommandMessage | 6 | 2 | SendCommandMessage |
| ProtocolMessage | sendCommandResultMessage | 7 | 2 | SendCommandResultMessage |
| ProtocolMessage | sendHIDEventMessage | 13 | 2 | SendHIDEventMessage |
| ProtocolMessage | sendPackedVirtualTouchEventMessage | 47 | 2 | SendPackedVirtualTouchEventMessage |
| ProtocolMessage | sendVoiceInputMessage | 36 | 2 | SendVoiceInputMessage |
| ProtocolMessage | setArtworkMessage | 10 | 2 | SetArtworkMessage |
| ProtocolMessage | setConnectionStateMessage | 42 | 2 | SetConnectionStateMessage |
| ProtocolMessage | setDefaultSupportedCommandsMessage | 75 | 2 | SetDefaultSupportedCommandsMessage |
| ProtocolMessage | setDiscoveryModeMessage | 82 | 2 | SetDiscoveryModeMessage |
| ProtocolMessage | setHiliteModeMessage | 44 | 2 | SetHiliteModeMessage |
| ProtocolMessage | setNowPlayingClientMessage | 50 | 2 | SetNowPlayingClientMessage |
| ProtocolMessage | setNowPlayingPlayerMessage | 51 | 2 | SetNowPlayingPlayerMessage |
| ProtocolMessage | setRecordingStateMessage | 35 | 2 | SetRecordingStateMessage |
| ProtocolMessage | setStateMessage | 9 | 2 | SetStateMessage |
| ProtocolMessage | setVolumeMessage | 55 | 2 | SetVolumeMessage |
| ProtocolMessage | textInputMessage | 30 | 2 | TextInputMessage |
| ProtocolMessage | transactionMessage | 38 | 2 | TransactionMessage |
| ProtocolMessage | updateClientMessage | 59 | 2 | UpdateClientMessage |
| ProtocolMessage | updateContentItemArtworkMessage | 61 | 2 | UpdateContentItemArtworkMessage |
| ProtocolMessage | updateContentItemMessage | 60 | 2 | UpdateContentItemMessage |
| ProtocolMessage | updateEndPointsMessage | 83 | 2 | UpdateEndPointsMessage |
| ProtocolMessage | updateOutputDeviceMessage | 69 | 2 | UpdateOutputDeviceMessage |
| ProtocolMessage | updatePlayerMessage | 62 | 2 | UpdatePlayerMessage |
| ProtocolMessage | volumeControlAvailabilityMessage | 22 | 2 | VolumeControlAvailabilityMessage |
| ProtocolMessage | volumeControlCapabilitiesDidChangeMessage | 68 | 2 | VolumeControlCapabilitiesDidChangeMessage |
| ProtocolMessage | volumeDidChangeMessage | 56 | 2 | VolumeDidChangeMessage |
| ProtocolMessage | wakeDeviceMessage | 45 | 2 | WakeDeviceMessage |
| RegisterForGameControllerEventsMessage | inputModeFlags | 1 | 0 | — |
| RegisterHIDDeviceMessage | deviceDescriptor | 1 | 2 | VirtualTouchDeviceDescriptor |
| RegisterHIDDeviceResultMessage | errorCode | 1 | 0 | — |
| RegisterHIDDeviceResultMessage | deviceIdentifier | 2 | 0 | — |
| RegisterVoiceInputDeviceMessage | deviceDescriptor | 1 | 2 | VoiceInputDeviceDescriptor |
| RegisterVoiceInputDeviceResponseMessage | deviceID | 1 | 0 | — |
| RegisterVoiceInputDeviceResponseMessage | errorCode | 2 | 0 | — |
| RemoteTextInputMessage | timestamp | 1 | 1 | — |
| RemoteTextInputMessage | version | 2 | 0 | — |
| RemoteTextInputMessage | data | 3 | 2 | — |
| RemoveClientMessage | client | 1 | 2 | NowPlayingClient |
| RemoveEndpointsMessage | endpointUIDs | 1 | 2 | — |
| RemoveOutputDevicesMessage | outputDeviceUIDs | 1 | 2 | — |
| RemoveOutputDevicesMessage | endpointUID | 2 | 2 | — |
| RemovePlayerMessage | playerPath | 1 | 2 | PlayerPath |
| SendButtonEventMessage | usagePage | 1 | 0 | — |
| SendButtonEventMessage | usage | 2 | 0 | — |
| SendButtonEventMessage | buttonDown | 3 | 0 | — |
| SendCommandMessage | command | 1 | 0 | — |
| SendCommandMessage | options | 2 | 2 | CommandOptions |
| SendCommandMessage | playerPath | 3 | 2 | PlayerPath |
| SendCommandResult | playerPath | 1 | 2 | PlayerPath |
| SendCommandResult | sendError | 2 | 0 | — |
| SendCommandResult | statuses | 3 | 2 | SendCommandResultStatus |
| SendCommandResultMessage | sendError | 1 | 0 | — |
| SendCommandResultMessage | handlerReturnStatus | 2 | 0 | — |
| SendCommandResultMessage | handlerReturnStatusDatas | 3 | 2 | — |
| SendCommandResultMessage | commandID | 4 | 2 | — |
| SendCommandResultMessage | playerPath | 5 | 2 | PlayerPath |
| SendCommandResultMessage | commandResult | 6 | 2 | SendCommandResult |
| SendCommandResultStatus | statusCode | 1 | 0 | — |
| SendCommandResultStatus | type | 2 | 0 | — |
| SendCommandResultStatus | customData | 5 | 2 | — |
| SendCommandResultStatus | customDataType | 6 | 2 | — |
| SendHIDEventMessage | hidEventData | 1 | 2 | — |
| SendPackedVirtualTouchEventMessage | data | 1 | 2 | — |
| SendVoiceInputMessage | dataBlock | 1 | 2 | AudioDataBlock |
| SetArtworkMessage | jpegData | 1 | 2 | — |
| SetConnectionStateMessage | state | 1 | 0 | — |
| SetDefaultSupportedCommandsMessage | nowPlayingInfo | 1 | 2 | NowPlayingInfo |
| SetDefaultSupportedCommandsMessage | supportedCommands | 2 | 2 | SupportedCommands |
| SetDefaultSupportedCommandsMessage | playbackQueue | 3 | 2 | PlaybackQueue |
| SetDefaultSupportedCommandsMessage | displayID | 4 | 2 | — |
| SetDefaultSupportedCommandsMessage | displayName | 5 | 2 | — |
| SetDefaultSupportedCommandsMessage | playbackState | 6 | 0 | — |
| SetDefaultSupportedCommandsMessage | playbackQueueCapabilities | 8 | 2 | PlaybackQueueCapabilities |
| SetDefaultSupportedCommandsMessage | playerPath | 9 | 2 | PlayerPath |
| SetDefaultSupportedCommandsMessage | request | 10 | 2 | PlaybackQueueRequestMessage |
| SetDefaultSupportedCommandsMessage | playbackStateTimestamp | 11 | 1 | — |
| SetDiscoveryModeMessage | mode | 1 | 0 | — |
| SetDiscoveryModeMessage | features | 2 | 0 | — |
| SetHiliteModeMessage | hiliteMode | 1 | 0 | — |
| SetNowPlayingClientMessage | client | 1 | 2 | NowPlayingClient |
| SetNowPlayingPlayerMessage | playerPath | 1 | 2 | PlayerPath |
| SetRecordingStateMessage | state | 1 | 0 | — |
| SetStateMessage | nowPlayingInfo | 1 | 2 | NowPlayingInfo |
| SetStateMessage | supportedCommands | 2 | 2 | SupportedCommands |
| SetStateMessage | playbackQueue | 3 | 2 | PlaybackQueue |
| SetStateMessage | displayID | 4 | 2 | — |
| SetStateMessage | displayName | 5 | 2 | — |
| SetStateMessage | playbackState | 6 | 0 | — |
| SetStateMessage | playbackQueueCapabilities | 8 | 2 | PlaybackQueueCapabilities |
| SetStateMessage | playerPath | 9 | 2 | PlayerPath |
| SetStateMessage | request | 10 | 2 | PlaybackQueueRequestMessage |
| SetStateMessage | playbackStateTimestamp | 11 | 1 | — |
| SetVolumeMessage | volume | 1 | 5 | — |
| SetVolumeMessage | outputDeviceUID | 2 | 2 | — |
| SupportedCommands | supportedCommands | 1 | 2 | CommandInfo |
| TextEditingAttributes | title | 1 | 2 | — |
| TextEditingAttributes | prompt | 2 | 2 | — |
| TextEditingAttributes | inputTraits | 3 | 2 | TextInputTraits |
| TextInputMessage | timestamp | 1 | 1 | — |
| TextInputMessage | text | 2 | 2 | — |
| TextInputMessage | actionType | 3 | 0 | — |
| TextInputTraits | autocapitalizationType | 1 | 0 | — |
| TextInputTraits | keyboardType | 2 | 0 | — |
| TextInputTraits | returnKeyType | 3 | 0 | — |
| TextInputTraits | autocorrection | 4 | 0 | — |
| TextInputTraits | spellchecking | 5 | 0 | — |
| TextInputTraits | enablesReturnKeyAutomatically | 6 | 0 | — |
| TextInputTraits | secureTextEntry | 7 | 0 | — |
| TextInputTraits | validTextRangeLocation | 8 | 0 | — |
| TextInputTraits | validTextRangeLength | 9 | 0 | — |
| TextInputTraits | pINEntrySeparatorIndexes | 10 | 0 | — |
| TransactionKey | identifier | 1 | 2 | — |
| TransactionKey | userData | 2 | 2 | — |
| TransactionMessage | name | 1 | 0 | — |
| TransactionMessage | packets | 2 | 2 | TransactionPackets |
| TransactionMessage | playerPath | 3 | 2 | PlayerPath |
| TransactionPacket | key | 1 | 2 | TransactionKey |
| TransactionPacket | packetData | 2 | 2 | — |
| TransactionPacket | identifier | 3 | 2 | — |
| TransactionPacket | totalLength | 4 | 0 | — |
| TransactionPacket | totalWritePosition | 5 | 0 | — |
| TransactionPackets | packets | 1 | 2 | TransactionPacket |
| UpdateClientMessage | client | 1 | 2 | NowPlayingClient |
| UpdateContentItemArtworkMessage | contentItems | 1 | 2 | ContentItem |
| UpdateContentItemArtworkMessage | playerPath | 2 | 2 | PlayerPath |
| UpdateContentItemMessage | contentItems | 1 | 2 | ContentItem |
| UpdateContentItemMessage | playerPath | 2 | 2 | PlayerPath |
| UpdateEndPointsMessage | endpoints | 1 | 2 | AVEndpointDescriptor |
| UpdateEndPointsMessage | endpointFeatures | 2 | 0 | — |
| UpdateOutputDeviceMessage | outputDevices | 1 | 2 | AVOutputDeviceDescriptor |
| UpdateOutputDeviceMessage | endpointUID | 2 | 2 | — |
| UpdateOutputDeviceMessage | clusterAwareOutputDevices | 3 | 2 | AVOutputDeviceDescriptor |
| UpdatePlayerMessage | playerPath | 1 | 2 | PlayerPath |
| VirtualTouchDeviceDescriptor | absolute | 1 | 0 | — |
| VirtualTouchDeviceDescriptor | integratedDisplay | 2 | 0 | — |
| VirtualTouchDeviceDescriptor | screenSizeWidth | 3 | 5 | — |
| VirtualTouchDeviceDescriptor | screenSizeHeight | 4 | 5 | — |
| VoiceInputDeviceDescriptor | defaultFormat | 1 | 2 | AudioFormatSettings |
| VoiceInputDeviceDescriptor | supportedFormats | 2 | 2 | AudioFormatSettings |
| VolumeControlAvailabilityMessage | volumeControlAvailable | 1 | 0 | — |
| VolumeControlAvailabilityMessage | volumeCapabilities | 2 | 0 | — |
| VolumeControlCapabilitiesDidChangeMessage | capabilities | 1 | 2 | VolumeControlAvailabilityMessage |
| VolumeControlCapabilitiesDidChangeMessage | endpointUID | 3 | 2 | — |
| VolumeControlCapabilitiesDidChangeMessage | outputDeviceUID | 4 | 2 | — |
| VolumeDidChangeMessage | volume | 1 | 5 | — |
| VolumeDidChangeMessage | endpointUID | 2 | 2 | — |
| VolumeDidChangeMessage | outputDeviceUID | 3 | 2 | — |
