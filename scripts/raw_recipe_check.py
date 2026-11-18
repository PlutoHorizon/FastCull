#!/usr/bin/env python3
"""Verify immutable recipe transport in managed RAW smoke runs (not GPU latency)."""
import argparse
from collections import Counter
import json
from pathlib import Path
import struct
import xml.etree.ElementTree as ET

CRS = '{http://ns.adobe.com/camera-raw-settings/1.0/}'
BASIC = ('Contrast2012', 'Highlights2012', 'Shadows2012', 'Whites2012',
         'Blacks2012', 'Vibrance', 'Saturation')
FRAME = ('recipe_session', 'recipe_revision', 'recipe_fingerprint',
         'requested_recipe_session', 'requested_recipe_revision',
         'scene_generation', 'resolved_profile_fingerprint', 'provisional')


def record(path):
    fields = path.read_text(encoding='utf-8').split('\n', 4)
    assert len(fields) == 5 and fields[0] == 'FastCullRecipeRecord1', path
    snapshot = fields[4]
    fingerprint = 0xcbf29ce484222325
    for byte in snapshot.encode('utf-8'):
        fingerprint = ((fingerprint ^ byte) * 0x100000001b3) & ((1 << 64) - 1)
    assert fields[3] == f'{fingerprint:016x}', f'{path}: corrupt fingerprint'
    values = snapshot.split('\n')
    assert len(values) == 6 and values[0].startswith('FastCullRecipe1:'), path
    assert all(value in ('0', '1') for value in values[4:]), path
    decoded = ['' if value == '~' else bytes.fromhex(value).decode('utf-8')
               for value in values[1:4]]
    attributes = {}
    for node in ET.fromstring(decoded[2]).iter():
        attributes.update(node.attrib)
    return (int(fields[1]), int(fields[2])), {
        'fingerprint': fields[3], 'pipeline': values[0].split(':', 1)[1],
        'attributes': attributes, 'path': str(path),
        'source': json.JSONDecoder().raw_decode(decoded[0])[0].split('|', 1)[0],
    }


def saved_attributes(path):
    if path.suffix.lower() != '.dng':
        packet = path.with_suffix('.xmp').read_bytes()
    else:
        # The managed fixture is classic TIFF. Read only the root-IFD XMP tag;
        # do not ask an image decoder to develop or rewrite the DNG.
        with path.open('rb') as handle:
            header = handle.read(8)
            assert header[:2] in (b'II', b'MM'), path
            endian = '<' if header[:2] == b'II' else '>'
            assert struct.unpack(endian + 'H', header[2:4])[0] == 42, path
            handle.seek(struct.unpack(endian + 'I', header[4:])[0])
            count = struct.unpack(endian + 'H', handle.read(2))[0]
            entries = handle.read(count * 12)
            packet = None
            for index in range(count):
                tag, kind, size, offset = struct.unpack_from(endian + 'HHII', entries, index * 12)
                if tag == 700:
                    assert kind in (1, 2, 7) and 4 < size <= 16 * 1024 * 1024, path
                    handle.seek(offset)
                    packet = handle.read(size)
                    assert len(packet) == size, path
                    break
            assert packet is not None, f'{path}: no embedded XMP'
    result = {}
    for node in ET.fromstring(packet.rstrip(b'\0')).iter():
        result.update(node.attrib)
    return result


def verify(root):
    events = sorted((json.loads(line)
                     for path in root.rglob('raw-trace-*.jsonl')
                     for line in path.read_text(encoding='utf-8').splitlines()),
                    key=lambda event: event['unix_us'])
    records = dict(record(path) for path in root.rglob('recipe-*.txt'))
    assert records and events, f'{root}: missing recipes or traces'
    stages = Counter(event['stage'] for event in events)
    required = ('render_requested', 'scene_ready', 'display_frame', 'histogram_recipe',
                'display_recipe_prepared', 'capture', 'save_done', 'save_receipt_queued')
    for stage in required:
        assert stages[stage], f'{root}: missing {stage}'
    versioned = 0
    for event in events:
        if 'recipe_revision' not in event:
            assert event['stage'] not in required, f'missing revision: {event}'
            continue
        key = (event['recipe_session'], event['recipe_revision'])
        recipe = records[key]
        assert event['recipe_fingerprint'] == recipe['fingerprint'], event
        assert event['recipe_pipeline'] == recipe['pipeline'], event
        versioned += 1
    def frame(event):
        return tuple(event[key] for key in FRAME)
    prepared = {frame(event) for event in events if event['stage'] == 'display_recipe_prepared'}
    for event in events:
        if event['stage'] in ('histogram_recipe', 'capture'):
            assert frame(event) in prepared, f'frame recipe not prepared: {event}'
    revisions = {}
    for event in events:
        if event['stage'] != 'edit_input':
            continue
        session, revision = event['recipe_session'], event['recipe_revision']
        assert revision >= revisions.get(session, 0), event
        revisions[session] = revision
        attributes = records[(session, revision)]['attributes']
        if 'exposure_uev' in event:
            assert abs(float(attributes[CRS + 'Exposure2012']) * 1e6
                       - float(event['exposure_uev'])) <= 1000, event
        if event.get('basic'):
            assert [int(attributes[CRS + key]) for key in BASIC] == [
                int(value) for value in event['basic'].split(',')], event
        if event.get('temperature'):
            assert int(attributes[CRS + 'Temperature']) == int(event['temperature']), event
            assert int(attributes[CRS + 'Tint']) == int(event['tint']), event
    successful = [event for event in events if event['stage'] == 'save_done' and event['ok']]
    assert successful, f'{root}: no successful metadata save'
    for event in successful:
        recipe = records[(event['recipe_session'], event['recipe_revision'])]
        attributes = recipe['attributes']
        assert abs(float(attributes[CRS + 'Exposure2012']) * 1e6
                   - event['exposure_uev']) <= 1000, event
        source = Path(recipe['source'])
        assert source.resolve().is_relative_to(root.resolve()), source
        actual = saved_attributes(source)
        for key in ('Exposure2012', 'Temperature', 'Tint', *BASIC):
            assert abs(float(attributes[CRS + key]) - float(actual[CRS + key])) <= 1e-6, (source, key)
        for key in ('CameraProfile', 'WhiteBalance'):
            if CRS + key in attributes:
                assert attributes[CRS + key] == actual[CRS + key], (source, key)
    final_captures = [event for event in events
                      if event['stage'] == 'capture' and event.get('name') == '03-saved.png']
    assert final_captures and all(not event['provisional'] for event in final_captures), root
    cleanup = [json.loads(path.read_text(encoding='utf-8')) for path in root.rglob('cleanup.json')]
    assert cleanup and all(item['ok'] for item in cleanup), f'{root}: cleanup incomplete'
    return {'root': str(root), 'ok': True, 'recipes': len(records),
            'versioned_events': versioned, 'stages': dict(stages),
            'metadata_saves': len(successful), 'final_captures': len(final_captures),
            'cleanup_receipts': len(cleanup)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', action='append', type=Path, required=True)
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    cases = []
    for root in args.root:
        try:
            cases.append(verify(root))
        except (AssertionError, KeyError, ValueError, OSError, struct.error, ET.ParseError) as error:
            cases.append({'root': str(root), 'ok': False, 'error': str(error)})
    result = {'schema': 'fastcull.raw_recipe.acceptance.v1', 'ok': all(c['ok'] for c in cases),
              'cases': cases,
              'limits': ['prepare events identify the recipe received by a render callback; they do not prove GPU completion or physical display latency.',
                         'save_done verifies metadata save intent; save_receipt_queued is not an acknowledgement of IPC receipt delivery.',
                         'Managed replay plus unit tests cover version transport; this is not a Lightroom match or long-duration stress test.']}
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return 0 if result['ok'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
