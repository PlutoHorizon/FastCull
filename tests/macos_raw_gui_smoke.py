"""Exercise the actual Mac RAW window with synthetic pixels and private metadata.

Runs only on a Mac desktop/CI runner. No Adobe software or user photos are used.
The app's existing opt-in smoke path edits, fits, loads full pixels and saves.
"""
import json
import os
from pathlib import Path
import subprocess
import sys
import xml.etree.ElementTree as ET

from macos_native_smoke import write_linear_dng


def main():
    if sys.platform != 'darwin':
        raise SystemExit('This test requires a macOS graphics session.')
    executable = Path(sys.argv[1]).resolve()
    directory = Path(sys.argv[2]).resolve()
    directory.mkdir(parents=True, exist_ok=False)
    source = directory / "合成 DNG ' $ sample.dng"
    write_linear_dng(source)
    original = source.read_bytes()
    request = directory / 'request.txt'
    receipt = directory / 'saved.txt'
    request.write_text('FASTCULL_RAW_WINDOW_V1\n' +
                       str(source).encode('utf-8').hex() + '\n' +
                       str(receipt).encode('utf-8').hex() + '\n-\n-\n', encoding='utf-8')
    env = {k:v for k,v in os.environ.items() if not k.startswith('FASTCULL_')}
    for variable, leaf in [('FASTCULL_RAW_EDIT_SMOKE_DIR', 'screenshots'),
                           ('FASTCULL_RAW_CACHE_DIR', 'cache'),
                           ('FASTCULL_LOG_DIR', 'logs')]:
        path = directory / leaf
        path.mkdir()
        env[variable] = str(path)
    subprocess.run([str(executable), '--raw-editor', str(request)],
                   env=env, check=True, timeout=120)
    screenshots = directory / 'screenshots'
    error = screenshots / 'error.txt'
    if error.exists():
        raise AssertionError(error.read_text(encoding='utf-8'))
    result = (screenshots / 'result.txt').read_text(encoding='utf-8')
    assert 'saved=true' in result, result
    assert len(list(screenshots.glob('*.png'))) == 4
    attrs = {key: value for node in ET.fromstring(source.with_suffix('.xmp').read_text(encoding='utf-8')).iter()
             for key, value in node.attrib.items()}
    prefix = '{http://ns.adobe.com/camera-raw-settings/1.0/}'
    expected = dict(Exposure2012='0.500000', WhiteBalance='Custom', Temperature='4850', Tint='17',
                    Contrast2012='20', Highlights2012='-35', Shadows2012='25', Whites2012='15',
                    Blacks2012='-10', Vibrance='20', Saturation='10')
    for key, value in expected.items():
        assert attrs[prefix + key] == value, (key, attrs.get(prefix + key))
    assert source.read_bytes()[8:len(original)] == original[8:], 'DNG payload bytes changed'
    assert len(list(directory.glob('saved-*.txt'))) == 1, 'Typed save receipt missing'
    logs='\n'.join(path.read_text(encoding='utf-8') for path in (directory/'logs').glob('*.log'))
    assert 'graphics backend=Metal' in logs, 'RAW window did not use Metal'
    assert 'graphics colorspace=sRGB metal_layers=' in logs, 'Metal layer has no sRGB color-space tag'
    from PIL import Image, ImageChops
    loaded=Image.open(screenshots/'00-loaded.png').convert('RGB')
    edited=Image.open(screenshots/'01-edited.png').convert('RGB')
    # Fractions keep the sample outside the right controls on Retina captures.
    photo=(int(loaded.width*.04),int(loaded.height*.15),int(loaded.width*.58),int(loaded.height*.8))
    pixels=loaded.crop(photo)
    data=pixels.get_flattened_data() if hasattr(pixels,'get_flattened_data') else pixels.getdata()
    assert len(set(data))>32, 'Synthetic DNG preview is blank'
    assert ImageChops.difference(loaded.crop(photo),edited.crop(photo)).getbbox(), 'Metal controls did not change pixels'
    report = dict(passed=True, synthetic_dng=True, gpu_screenshots=4,
                  raw_payload_preserved=True, typed_receipt=True,
                  xmp_fields=len(expected), adobe_required=False,backend='Metal',srgb_layer_tagged=True,
                  preview_has_pixels=True,controls_change_pixels=True)
    (directory / 'report.json').write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    print('macOS RAW GUI:', json.dumps(report))


if __name__ == '__main__':
    main()
