"""The two measured local-SDK titlebar offsets, never a whole-frame waiver."""
import re
from pathlib import Path

from cleanup import bounded_command


def exceptions(old_sdk, new_sdk, before, after):
    """Only published SDK26.5 y=7 -> local SDK15.0 y=8 is explained here."""
    if (old_sdk, new_sdk) != ('26.5', '15.0'):
        return []
    found = []
    for name in ('save_button', 'sidebar_button'):
        old = before.get('app', {}).get(name, {}).get('frame', [])
        new = after.get('app', {}).get(name, {}).get('frame', [])
        if isinstance(old, list) and isinstance(new, list) and len(old) == len(new) == 4 and old[1] == 7 and new[1] == 8:
            found.append((f'app.{name}.frame[1]', 'measured local SDK15.0 titlebar y=8 versus published SDK26.5 y=7; all other coordinates still compared'))
    return found


def linked_sdk(app):
    code, raw, error = bounded_command(['/usr/bin/vtool', '-show-build', str(Path(app) / 'Contents/MacOS/RectoPDF')])
    values = re.findall(rb'^\s+sdk (\d+(?:\.\d+)+)\s*$', raw, re.MULTILINE)
    if code != 0 or len(values) != 1:
        raise RuntimeError('cannot establish linked SDK for titlebar comparison: ' + error.decode(errors='replace')[:512])
    return values[0].decode('ascii')


def comparison(old_app, new_app, before, after):
    old, new = linked_sdk(old_app), linked_sdk(new_app)
    return {'old_linked_sdk': old, 'new_linked_sdk': new, 'excepted': exceptions(old, new, before, after)}
