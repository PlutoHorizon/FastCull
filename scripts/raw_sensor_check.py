#!/usr/bin/env python3
"""Headless same-stage LibRaw/CPU/GPU sensor acceptance on private RAW fixtures."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess


def digest(path):
    result=hashlib.sha256()
    with path.open('rb') as handle:
        for block in iter(lambda:handle.read(1<<20),b''):
            result.update(block)
    return result.hexdigest()


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--probe',type=Path,required=True)
    parser.add_argument('--data-root',type=Path,required=True)
    parser.add_argument('--report',type=Path,required=True)
    parser.add_argument('--gpu',action='store_true')
    parser.add_argument('--formats',default='arw,dng,nef')
    args=parser.parse_args()
    probe=args.probe.resolve()
    dll=probe.with_name('fastcull_raw.dll' if __import__('os').name=='nt' else 'libfastcull_raw.dylib')
    rows=[]
    for fmt in args.formats.split(','):
        source=args.data_root/f'probe.{fmt}'
        before=digest(source)
        process=subprocess.run([str(probe),str(source.resolve()),*(['--gpu'] if args.gpu else [])],
                               capture_output=True,text=True,encoding='utf-8',errors='replace',timeout=120)
        events=[json.loads(line) for line in process.stdout.splitlines() if line.startswith('{')]
        comparisons=[event for event in events if event.get('backend')]
        passed=(process.returncode==0 and before==digest(source)
                and len(comparisons)==(2 if args.gpu else 1)
                and all(event.get('all_metadata_equal') and event.get('all_samples_equal') for event in comparisons)
                and any(event.get('cancellation_rejected') for event in events))
        rows.append({'format':fmt,'source_sha256':before,'source_unchanged':before==digest(source),
                     'ok':passed,'exit_code':process.returncode,'events':events,'stderr':process.stderr})
    result={'schema':'fastcull.raw_sensor.acceptance.v1','stage':'libraw-unpack-u16-v1',
            'probe_sha256':digest(probe),'dll_sha256':digest(dll),
            'ok':all(row['ok'] for row in rows),'cases':rows,
            'limits':['Native checkpoint is diagnostic-only; no RAW or XMP writes and no application windows.',
                      'Integer samples are compared after unpack, before raw2image, sensor WB, AHD or display; linear DNG includes decoder-managed curve mapping.',
                      'Canonical data excludes row padding and unused fourth RGB component; full black pattern/curve/WB/geometry metadata must also match.',
                      'NEF fallback is not custom CPU/GPU entropy coverage; no Lightroom or performance superiority acceptance.']}
    args.report.parent.mkdir(parents=True,exist_ok=True)
    args.report.write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
    print(json.dumps(result,ensure_ascii=False,indent=2))
    return 0 if result['ok'] else 1


if __name__=='__main__':
    raise SystemExit(main())
