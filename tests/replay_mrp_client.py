"""Replay Rust-generated protobuf fixtures through pyatv, without connections."""
import json
import sys
import socket
from pathlib import Path

from pyatv.protocols.mrp import protobuf as pb
from pyatv.protocols.mrp.player_state import PlayerState

def deny_connection(*args, **kwargs):
    raise AssertionError('Network connections are forbidden in this client replay')

socket.socket.connect = deny_connection
socket.socket.connect_ex = deny_connection
socket.create_connection = deny_connection

if len(sys.argv) != 2:
    raise SystemExit('Usage: replay_mrp_client.py FIXTURE_DIRECTORY (requires pyatv)')
root = Path(sys.argv[1])

def load(name):
    message = pb.ProtocolMessage()
    message.ParseFromString((root / (name + '.pb')).read_bytes())
    return pb.extract_inner(message)

def player():
    result = PlayerState(None, pb.NowPlayingPlayer(identifier='test-player'))
    result.handle_set_state(load('initial'))
    return result

# Negative control: the actual old candidate's shape discards existing fields.
broken = player()
assert broken.metadata.title == 'Song'
broken.handle_set_state(load('legacy_pause'))
assert not broken.metadata.HasField('title')
assert not broken.metadata.HasField('artworkIdentifier')

current = player()
initial_commands = [item.command for item in current.supported_commands]
initial_archive = current.metadata.nowPlayingInfoData
assert initial_commands and current.metadata.artworkIdentifier == 'cover'
for prefix, state, rate in [('pause', pb.PlaybackState.Paused, 0.0),
                            ('play', pb.PlaybackState.Playing, 1.0)]:
    current.handle_set_state(load(prefix + '_state'))
    current.handle_content_item_update(load(prefix + '_item'))
    assert current.playback_state == state
    assert current.metadata.playbackRate == rate
    assert current.metadata.title == 'Song'
    assert current.metadata.artworkIdentifier == 'cover'
    assert current.metadata.artworkAvailable
    assert current.metadata.nowPlayingInfoData == initial_archive
    assert [item.command for item in current.supported_commands] == initial_commands
    assert current.item_identifier == 'track'
    assert len(current.items) == 1
    assert abs(current.metadata.elapsedTime - 12.0) < 1e-6


# Replay the complete multi-message packet through the unmodified pyatv decoder.
from pyatv.protocols.airplay.channels import BaseDataStreamChannel
from pyatv.support.variant import read_variant, write_variant
import logging
packet = (root / 'group_update_packet.bin').read_bytes()
decoded = BaseDataStreamChannel.decode_protobufs(packet)
assert len(decoded) == 5
assert decoded[0].type == pb.DEVICE_INFO_UPDATE_MESSAGE
assert any(message.type == pb.UPDATE_OUTPUT_DEVICE_MESSAGE for message in decoded)
length, rest = read_variant(packet)
legacy = (root / 'legacy_device_update.pb').read_bytes()
bad_packet = write_variant(len(legacy)) + legacy + rest[length:]
previous_logging = logging.root.manager.disable
try:
    logging.disable(logging.CRITICAL)  # Expected negative-control decoder error.
    assert BaseDataStreamChannel.decode_protobufs(bad_packet) == []
finally:
    logging.disable(previous_logging)

print(json.dumps({'old_shape_loses_metadata': True,
                  'new_shape_preserves_queue_artwork_commands': True,
                  'pause_play_correct': True,
                  'network_connections': 0, 'strict_group_packet_decoded': True, 'old_order_drops_following_updates': True}))
