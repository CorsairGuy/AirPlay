"""Проверка приёмника независимым клиентом: криптографией и сопряжением pyatv.

Запуск (нужны pyatv и srptools 1.0.1, на которую рассчитан pyatv):

    pip install pyatv "srptools==1.0.1"
    airplay serve --port 7123 --pin 1234 --no-mdns &
    python tests/interop/pyatv_client.py 127.0.0.1 7123 1234
"""

import binascii
import plistlib
import socket
import sys

from pyatv.auth import hap_tlv8
from pyatv.auth.hap_session import HAPSession
from pyatv.auth.hap_srp import SRPAuthHandler, hkdf_expand
from pyatv.auth.hap_tlv8 import TlvValue
from pyatv.protocols.airplay.srp import LegacySRPAuthHandler, new_credentials

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
    c.enable(binascii.unhexlify(srp.shared_key))
    print("transient: ок, /info по шифрованному каналу:", info(c)["name"])


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


print("/info:", {k: info(Conn())[k] for k in ("name", "deviceID", "features", "pi")})
transient()
pin_setup_and_verify()
legacy()
print("все проверки пройдены")
