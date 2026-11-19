"""Guard the observed SDK offset without concealing a changed button frame."""
import copy
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from titlebar import exceptions


class Titlebar(unittest.TestCase):
    # Failure: a local dress rehearsal refuses the measured SDK offset, or a
    # broad waiver conceals a real position/size change in the signed draft.
    def test_only_the_measured_sdk_and_y_transition_is_excepted(self):
        before = {'app': {n: {'frame': [10, 7, 24, 24]} for n in ('save_button', 'sidebar_button')}}
        after = copy.deepcopy(before)
        for value in after['app'].values():
            value['frame'][1] = 8
        paths = [p for p, _ in exceptions('26.5', '15.0', before, after)]
        self.assertEqual(paths, ['app.save_button.frame[1]', 'app.sidebar_button.frame[1]'])
        for sdk in ('26.5', '27.2', 'unknown'):
            self.assertEqual(exceptions('26.5', sdk, before, after), [])
        after['app']['save_button']['frame'][1] = 9
        self.assertEqual([p for p, _ in exceptions('26.5', '15.0', before, after)], ['app.sidebar_button.frame[1]'])
        self.assertFalse(any(p.endswith('frame') for p in paths), 'x, width and height must remain compared')


if __name__ == '__main__':
    unittest.main()
