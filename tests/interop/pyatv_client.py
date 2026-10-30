"""Проверка приёмника независимым клиентом: криптографией, сопряжением и
MRP из pyatv.

Запуск (нужны pyatv и srptools 1.0.1, на которую рассчитан pyatv):

    pip install pyatv "srptools==1.0.1"
    airplay serve --port 7123 --pin 1234 --no-mdns --mrp &
    python tests/interop/pyatv_client.py 127.0.0.1 7123 1234
"""

import asyncio
import binascii
import plistlib
import random
import socket
import sys
import uuid

from pyatv.auth import hap_tlv8
from pyatv.auth.hap_session import HAPSession
from pyatv.auth.hap_srp import SRPAuthHandler, hkdf_expand
from pyatv.auth.hap_tlv8 import TlvValue
from pyatv.protocols.airplay.channels import DataStreamChannel
from pyatv.protocols.airplay.srp import LegacySRPAuthHandler, new_credentials
from pyatv.protocols.mrp import messages, protobuf
from pyatv.settings import InfoSettings

HOST, PORT, PIN = sys.argv[1], int(sys.argv[2]), sys.argv[3]


class Conn:
    def __init__(self):
        self.sock = socket.create_connection((HOST, PORT), timeout=5)
        self.session = HAPSession()
        self.cseq = 0
        self.buf = b""

    def call(self, method, uri, body=b"", headers=None):
        self.cseq += 1
        head = f"{method} {uri} RTSP/1.0\r\nCSeq: {self.cseq}\r\n"
        for k, v in (headers or {}).items():
            head += f"{k}: {v}\r\n"
        if body:
            head += f"Content-Length: {len(body)}\r\n"
        self.sock.sendall(self.session.encrypt(head.encode() + b"\r\n" + body))
        while True:
            if b"\r\n\r\n" in self.buf:
                head, rest = self.buf.split(b"\r\n\r\n", 1)
                lines = head.decode().split("\r\n")
                hdrs = {k.lower(): v.strip() for k, v in (l.split(":", 1) for l in lines[1:])}
                n = int(hdrs.get("content-length", 0))
                if len(rest) >= n:
                    self.buf = rest[n:]
                    status = int(lines[0].split()[1])
                    assert hdrs["cseq"] == str(self.cseq), hdrs
                    return status, rest[:n]
            data = self.sock.recv(65536)
            assert data, "соединение закрыто"
            self.buf += self.session.decrypt(data)

    def enable(self, shared):
        out = hkdf_expand("Control-Salt", "Control-Write-Encryption-Key", shared)
        inp = hkdf_expand("Control-Salt", "Control-Read-Encryption-Key", shared)
        self.session.enable(out, inp)


def info(c):
    status, body = c.call("GET", "/info")
    assert status == 200, status
    return plistlib.loads(body)


def transient():
    c = Conn()
    srp = SRPAuthHandler()
    srp.initialize()
    hkp = {"X-Apple-HKP": "4"}
    assert c.call("POST", "/pair-pin-start", headers=hkp)[0] == 200
    m1 = {TlvValue.Method: b"\x00", TlvValue.SeqNo: b"\x01", TlvValue.Flags: b"\x10"}
    m2 = hap_tlv8.read_tlv(c.call("POST", "/pair-setup", hap_tlv8.write_tlv(m1), hkp)[1])
    srp.step1(3939)
    pub, proof = srp.step2(m2[TlvValue.PublicKey], m2[TlvValue.Salt])
    m3 = {TlvValue.SeqNo: b"\x03", TlvValue.PublicKey: pub, TlvValue.Proof: proof}
    m4 = hap_tlv8.read_tlv(c.call("POST", "/pair-setup", hap_tlv8.write_tlv(m3), hkp)[1])
    assert TlvValue.Error not in m4 and TlvValue.Proof in m4, m4
    shared = binascii.unhexlify(srp.shared_key)
    c.enable(shared)
    print("transient: ок, /info по шифрованному каналу:", info(c)["name"])
    return c, shared


def pin_setup_and_verify():
    c = Conn()
    srp = SRPAuthHandler()
    srp.initialize()
    hkp = {"X-Apple-HKP": "3"}
    m1 = {TlvValue.Method: b"\x00", TlvValue.SeqNo: b"\x01"}
    m2 = hap_tlv8.read_tlv(c.call("POST", "/pair-setup", hap_tlv8.write_tlv(m1), hkp)[1])
    srp.step1(PIN)
    pub, proof = srp.step2(m2[TlvValue.PublicKey], m2[TlvValue.Salt])
    m3 = {TlvValue.SeqNo: b"\x03", TlvValue.PublicKey: pub, TlvValue.Proof: proof}
    m4 = hap_tlv8.read_tlv(c.call("POST", "/pair-setup", hap_tlv8.write_tlv(m3), hkp)[1])
    assert TlvValue.Error not in m4, m4
    m5 = {TlvValue.SeqNo: b"\x05", TlvValue.EncryptedData: srp.step3()}
    m6 = hap_tlv8.read_tlv(c.call("POST", "/pair-setup", hap_tlv8.write_tlv(m5), hkp)[1])
    creds = srp.step4(m6[TlvValue.EncryptedData])
    print("pair-setup по PIN: ок, id приёмника", bytes(creds.atv_id).decode())

    c = Conn()
    v = SRPAuthHandler()
    _, verify_pub = v.initialize()
    m1 = {TlvValue.SeqNo: b"\x01", TlvValue.PublicKey: verify_pub}
    m2 = hap_tlv8.read_tlv(c.call("POST", "/pair-verify", hap_tlv8.write_tlv(m1), hkp)[1])
    enc = v.verify1(creds, m2[TlvValue.PublicKey], m2[TlvValue.EncryptedData])
    m3 = {TlvValue.SeqNo: b"\x03", TlvValue.EncryptedData: enc}
    m4 = hap_tlv8.read_tlv(c.call("POST", "/pair-verify", hap_tlv8.write_tlv(m3), hkp)[1])
    assert TlvValue.Error not in m4, m4
    c.enable(v._shared)
    print("pair-verify: ок, /info по шифрованному каналу:", info(c)["name"])


def legacy():
    c = Conn()
    srp = LegacySRPAuthHandler(new_credentials())
    srp.initialize()
    status, server_pk = c.call("POST", "/pair-setup", srp._auth_public)
    assert status == 200 and len(server_pk) == 32
    status, reply = c.call("POST", "/pair-verify", srp.verify1())
    assert status == 200 and len(reply) == 96
    status, _ = c.call("POST", "/pair-verify", srp.verify2(reply[:32], reply[32:]))
    assert status == 200, status
    print("legacy pair-setup и pair-verify: ок")


def setup(c, body):
    status, reply = c.call("SETUP", f"rtsp://{HOST}/{random.getrandbits(32)}", plistlib.dumps(body, fmt=plistlib.FMT_BINARY),
                           {"Content-Type": "application/x-apple-binary-plist"})
    assert status == 200, status
    return plistlib.loads(reply)


class Listener:
    def __init__(self):
        self.queue = asyncio.Queue()

    def handle_protobuf(self, message):
        self.queue.put_nowait(message)

    def handle_connection_lost(self, exc):
        pass


async def mrp_session(port, out_key, in_key):
    channel = DataStreamChannel(out_key, in_key)
    listener = Listener()
    channel.listener = listener
    transport, _ = await asyncio.get_running_loop().create_connection(lambda: channel, HOST, port)

    async def receive(kind, identifier=None):
        while True:
            m = await asyncio.wait_for(listener.queue.get(), 3)
            if m.type == kind and (identifier is None or m.identifier == identifier):
                return m

    try:
        request = messages.device_information(InfoSettings(), "pyatv")
        request.identifier = "info"
        channel.send_protobuf(request)
        device = protobuf.extract_inner(await receive(protobuf.DEVICE_INFO_MESSAGE, "info"))
        assert device.modelID and device.deviceUID, device

        channel.send_protobuf(messages.client_updates_config())
        out = protobuf.extract_inner(await receive(protobuf.UPDATE_OUTPUT_DEVICE_MESSAGE))
        output = out.clusterAwareOutputDevices[0]
        assert output.uniqueIdentifier == device.deviceUID and list(output.pickable) == [True], output
        state = protobuf.extract_inner(await receive(protobuf.SET_STATE_MESSAGE))
        await receive(protobuf.SET_NOW_PLAYING_PLAYER_MESSAGE)

        volume = messages.create(protobuf.GET_VOLUME_MESSAGE, identifier="volume")
        channel.send_protobuf(volume)
        await receive(protobuf.GET_VOLUME_RESULT_MESSAGE, "volume")

        bad = messages.create(protobuf.SET_VOLUME_MESSAGE, identifier="bad-volume")
        protobuf.extract_inner(bad).volume = 1.5
        channel.send_protobuf(bad)
        assert (await receive(0, "bad-volume")).errorCode == 18

        queue = messages.playback_queue_request(0)
        queue.identifier = "queue"
        body = protobuf.extract_inner(queue)
        body.playerPath.CopyFrom(state.playerPath)
        body.requestID = "queue-id"
        channel.send_protobuf(queue)
        reply = protobuf.extract_inner(await receive(protobuf.SET_STATE_MESSAGE, "queue"))
        assert reply.playbackQueue.requestId == "queue-id", reply

        unknown = messages.command(protobuf.CommandInfo_pb2.ChangeRepeatMode)
        unknown.identifier = "repeat"
        channel.send_protobuf(unknown)
        result = protobuf.extract_inner(await receive(protobuf.SEND_COMMAND_RESULT_MESSAGE, "repeat"))
        assert result.sendError == protobuf.SendError.NotSupported, result
        return device.name
    finally:
        transport.close()


def mrp():
    """Как pyatv управляет HomePod: сессия пульта и MRP по потоку 130."""
    c, shared = transient()
    setup(c, {"isRemoteControlOnly": True, "timingProtocol": "None", "sourceVersion": "550.10",
              "sessionUUID": str(uuid.uuid4()).upper(), "name": "pyatv"})
    assert c.call("RECORD", f"rtsp://{HOST}/1")[0] == 200
    seed = random.getrandbits(64)
    reply = setup(c, {"streams": [{"type": 130, "controlType": 2, "seed": seed, "wantsDedicatedSocket": True,
                                   "channelID": str(uuid.uuid4()).upper(), "clientUUID": str(uuid.uuid4()).upper(),
                                   "clientTypeUUID": "1910A70F-DBC0-4242-AF95-115DB30604E1"}]})
    port = reply["streams"][0]["dataPort"]
    salt = f"DataStream-Salt{seed}"
    out_key = hkdf_expand(salt, "DataStream-Output-Encryption-Key", shared)
    in_key = hkdf_expand(salt, "DataStream-Input-Encryption-Key", shared)
    name = asyncio.run(mrp_session(port, out_key, in_key))
    print("MRP по потоку 130: ок, описание устройства, выход, карточка, громкость, очередь, ошибки команд:", name)


print("/info:", {k: info(Conn())[k] for k in ("name", "deviceID", "features", "pi")})
transient()
pin_setup_and_verify()
legacy()
mrp()
print("все проверки пройдены")
