"""Measure Bridge request-to-process and first-frame latency from retained logs."""
import argparse
import json
import re
from pathlib import Path


def analyze(directory):
    launchers, guis = [], []
    for path in directory.glob('fastcull_app_*.log'):
        lines = path.read_text(encoding='utf-8', errors='replace').splitlines()
        if not lines:
            continue
        header = lines[0]
        entry = re.search(r'startup entry unix_ms=(\d+)', '\n'.join(lines[:4]))
        if not entry:
            continue
        epoch = int(entry[1])
        if '"--bridge-open"' in header:
            request = re.search(r'bridge-requests.*?(\d{13})-\d+\.txt', header)
            if request:
                launchers.append({'request_ms': int(request[1]), 'entry_ms': epoch,
                                  'launcher_log': path.name})
        elif '"--bridge-list-owned"' in header:
            record = {'entry_ms': epoch, 'gui_log': path.name}
            request = re.search(r'bridge-requests.*?(\d{13})-\d+\.starting', header)
            if request:
                record['request_ms'] = int(request[1])
            for line in lines:
                for marker, field in [('startup shell-first-frame', 'shell_ms'),
                                      ('first UI frame ', 'frame_ms'),
                                      ('first loupe ', 'image_ms')]:
                    if marker in line:
                        record.setdefault(field, int(line.split()[0]))
            guis.append(record)
    results = []
    for gui in guis:
        if 'request_ms' in gui:
            row = {'request_ms': gui['request_ms'], 'entry_ms': gui['entry_ms'],
                   'gui_log': gui['gui_log'], 'route': 'native-queue',
                   'request_to_entry_ms': gui['entry_ms'] - gui['request_ms']}
            for field in ['shell_ms', 'frame_ms', 'image_ms']:
                if field in gui:
                    row['request_to_' + field] = gui[field] - gui['request_ms']
                    row['gui_to_' + field] = gui[field] - gui['entry_ms']
            results.append(row)
    for launch in sorted(launchers, key=lambda item: item['entry_ms']):
        gui = min((item for item in guis if 0 <= item['entry_ms'] - launch['entry_ms'] < 1000),
                  key=lambda item: item['entry_ms'], default={})
        row = dict(launch)
        row['request_to_entry_ms'] = launch['entry_ms'] - launch['request_ms']
        if gui:
            row['gui_log'] = gui['gui_log']
            row['launcher_to_gui_ms'] = gui['entry_ms'] - launch['entry_ms']
            for field in ['shell_ms', 'frame_ms', 'image_ms']:
                if field in gui:
                    row['request_to_' + field] = gui[field] - launch['request_ms']
                    row['gui_to_' + field] = gui[field] - gui['entry_ms']
        results.append(row)
    return sorted(results, key=lambda row: row['request_ms'])


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--logs', type=Path, required=True)
    parser.add_argument('--last', type=int, default=4)
    parser.add_argument('--max-handoff-ms', type=float)
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    rows = analyze(args.logs)[-args.last:]
    assert rows, 'No Bridge startup logs found'
    report = json.dumps(rows, ensure_ascii=False, indent=2)
    print(report)
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(report, encoding='utf-8')
    if args.max_handoff_ms is not None:
        assert max(row['request_to_entry_ms'] for row in rows) < args.max_handoff_ms, \
            'Bridge request-to-process latency exceeds budget'
