#!/usr/bin/env python3
"""Verification bout en bout du protocole contre un serveur en cours d'execution.

Deliberement ecrit sans dependance et sans reutiliser hwarang-protocol : un test
qui partagerait l'implementation de l'encodage ne prouverait rien sur le format
reellement emis sur le fil.

Usage : scripts/smoke.py [hote] [port]
"""

import socket
import struct
import sys

HANDSHAKE = 0x01
PING = 0x02
HANDSHAKE_ACCEPTED = 0x81
HANDSHAKE_REJECTED = 0x82
PONG = 0x83

# Doit suivre PROTOCOL_VERSION cote Rust. Un oubli se manifeste par un
# HandshakeRejected — ce qui est precisement le comportement attendu du serveur.
PROTOCOL_VERSION = 3


def frame(opcode: int, payload: bytes) -> bytes:
    return struct.pack(">H", len(payload) + 1) + bytes([opcode]) + payload


def read_frame(sock: socket.socket) -> tuple[int, bytes]:
    header = sock.recv(2)
    if len(header) < 2:
        raise ConnectionError("connexion fermee avant l'en-tete")
    length = struct.unpack(">H", header)[0]
    body = sock.recv(length)
    return body[0], body[1:]


def connect(host: str, port: int) -> socket.socket:
    return socket.create_connection((host, port), timeout=3)


def check_nominal(host: str, port: int) -> None:
    sock = connect(host, port)
    sock.sendall(frame(HANDSHAKE, struct.pack(">H", PROTOCOL_VERSION)))
    opcode, payload = read_frame(sock)
    assert opcode == HANDSHAKE_ACCEPTED, f"handshake refuse (0x{opcode:02x})"
    print(f"  handshake accepte, session {struct.unpack('>Q', payload)[0]}")

    nonce = 0xDEADBEEF
    sock.sendall(frame(PING, struct.pack(">I", nonce)))
    opcode, payload = read_frame(sock)
    assert opcode == PONG, f"pong attendu, recu 0x{opcode:02x}"
    assert struct.unpack(">I", payload)[0] == nonce, "nonce non renvoye a l'identique"
    print(f"  pong 0x{nonce:08X}")
    sock.close()


def check_version_mismatch(host: str, port: int) -> None:
    sock = connect(host, port)
    sock.sendall(frame(HANDSHAKE, struct.pack(">H", PROTOCOL_VERSION + 98)))
    opcode, payload = read_frame(sock)
    assert opcode == HANDSHAKE_REJECTED, f"rejet attendu, recu 0x{opcode:02x}"
    print(f"  version incompatible refusee, serveur en v{struct.unpack('>H', payload)[0]}")
    sock.close()


def check_out_of_sequence(host: str, port: int) -> None:
    sock = connect(host, port)
    sock.sendall(frame(PING, struct.pack(">I", 1)))
    assert sock.recv(16) == b"", "le serveur a repondu avant le handshake"
    print("  ping avant handshake : connexion fermee")
    sock.close()


def check_oversized_frame(host: str, port: int) -> None:
    sock = connect(host, port)
    sock.sendall(struct.pack(">H", 60000) + bytes([PING]))
    assert sock.recv(16) == b"", "le serveur a accepte une trame surdimensionnee"
    print("  trame surdimensionnee : connexion fermee")
    sock.close()


def main() -> int:
    host = sys.argv[1] if len(sys.argv) > 1 else "127.0.0.1"
    port = int(sys.argv[2]) if len(sys.argv) > 2 else 13000

    print(f"smoke {host}:{port}")
    for check in (
        check_nominal,
        check_version_mismatch,
        check_out_of_sequence,
        check_oversized_frame,
    ):
        try:
            check(host, port)
        except (AssertionError, OSError) as error:
            print(f"ECHEC {check.__name__} : {error}", file=sys.stderr)
            return 1

    print("OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
