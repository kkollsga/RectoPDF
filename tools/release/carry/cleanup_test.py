"""Made carry cleanup failures; no apps, OS queries, real sleeps or signals."""
import os
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import cleanup


class Scenario:
    def __init__(self, observations=None, registrations=None, unregister_code=0):
        self.time = 0
        self.events = []
        self.observations = list(observations or [{'live': {}, 'unknown': []}])
        self.registrations = list(registrations or [False])
        self.unregister_code = unregister_code
        self.bundle = {'path': '/private/carry/new/RectoPDF.app', 'id': 'test.RectoPDF'}
        self.run = SimpleNamespace(state='/private/carry/state', pid=123)

    def probe(self):
        self.events.append('probe')
        return self.observations.pop(0) if len(self.observations) > 1 else self.observations[0]

    def quit(self, run):
        self.events.append(('quit', run.state))
        return SimpleNamespace(returncode=0)

    def pause(self, seconds):
        self.events.append('poll')
        self.time += seconds

    def forget(self, bundle):
        self.events.append('unregister')
        return self.unregister_code

    def query(self, bundle):
        self.events.append('query')
        value = self.registrations.pop(0) if len(self.registrations) > 1 else self.registrations[0]
        if isinstance(value, Exception):
            raise value
        return value

    def finish(self, runs=None):
        return cleanup.finish('/private/carry', [self.bundle], runs or [self.run], self.probe,
                              self.quit, self.forget, self.query, lambda: self.events.append('remove'),
                              now=lambda: self.time, pause=self.pause, limit=0.1)


class CleanupTests(unittest.TestCase):
    def test_a_missing_mapped_executable_is_not_process_exit(self):
        """macOS ENOENT can describe a still-live process with an unlinked image."""
        self.assertTrue(cleanup.process_vanished(cleanup.errno.ESRCH))
        self.assertFalse(cleanup.process_vanished(cleanup.errno.ENOENT))


    def test_known_nested_helper_urls_are_saved_without_a_recursive_sweep(self):
        """Forgetting only the outer app must not leave its renderer helper registered."""
        apps = ['/private/carry/old/RectoPDF.app', '/private/carry/new/RectoPDF.app']
        with patch.object(cleanup.Path, 'resolve', lambda path: path), \
                patch.object(cleanup.Path, 'is_dir', return_value=True), \
                patch.object(cleanup.Path, 'stat', return_value=SimpleNamespace(st_size=100)), \
                patch.object(cleanup.Path, 'read_bytes', return_value=cleanup.plistlib.dumps({'CFBundleIdentifier': 'test.fixture'})):
            bundles = cleanup.describe('/private/carry', apps)
        self.assertEqual([bundle['path'] for bundle in bundles],
                         [path for app in apps for path in [app, app + '/Contents/Helpers/RectoRender.app']])
        self.assertEqual(len(bundles), 4)

    def test_os_query_pipes_close_even_if_owned_child_reaping_fails(self):
        """A timed-out OS query must release its pipes even if kill/wait also fails."""
        child, selector = Mock(), Mock()
        streams = {stream: SimpleNamespace(fileobj=stream) for stream in [child.stdout, child.stderr]}
        selector.get_map.return_value = streams
        selector.unregister.side_effect = lambda stream: streams.pop(stream)
        child.wait.side_effect = TimeoutError('owned query did not reap')
        with patch.object(cleanup.subprocess, 'Popen', return_value=child), \
                patch.object(cleanup.selectors, 'DefaultSelector', return_value=selector), \
                patch.object(cleanup.os, 'set_blocking'):
            with self.assertRaisesRegex(TimeoutError, 'OS-query command exceeded'):
                cleanup.bounded_command(['made OS query'], timeout=0)
        child.stdout.close.assert_called_once()
        child.stderr.close.assert_called_once()
        selector.close.assert_called_once()

    def test_os_query_setup_failure_still_retires_owned_child_and_preserves_error(self):
        """Failure before pipe registration must not strand the query child or replace its cause."""
        child, selector = Mock(), Mock()
        selector.get_map.return_value = {}
        selector.register.side_effect = RuntimeError('selector setup failed')
        child.wait.side_effect = TimeoutError('reaping also failed')
        with patch.object(cleanup.subprocess, 'Popen', return_value=child), \
                patch.object(cleanup.selectors, 'DefaultSelector', return_value=selector), \
                patch.object(cleanup.os, 'set_blocking'):
            with self.assertRaisesRegex(RuntimeError, 'selector setup failed'):
                cleanup.bounded_command(['made OS query'])
        child.kill.assert_called_once()
        child.wait.assert_called_once()
        child.stdout.close.assert_called_once()
        child.stderr.close.assert_called_once()
        selector.close.assert_called_once()

    def query_result(self, array, error, osstatus):
        cf, ls = Mock(), Mock()
        cf.CFStringCreateWithCString.return_value = 11
        cf.CFErrorGetCode.return_value = -10814
        cf.CFArrayGetCount.return_value = 0
        def lookup(key, error_pointer):
            error_pointer._obj.value = error
            return array
        ls.LSCopyApplicationURLsForBundleIdentifier.side_effect = lookup
        with patch.object(cleanup.ctypes, 'CDLL', side_effect=[cf, ls]), \
                patch.object(cleanup, '_osstatus_domain', return_value=osstatus, create=True):
            return cleanup.registration_query('test.fixture', '/private/carry/new/RectoPDF.app')

    def test_not_found_code_in_another_cf_domain_is_not_absence(self):
        """An unrelated CF error can share -10814; only OSStatus declares app-not-found."""
        with self.assertRaises(RuntimeError):
            self.query_result(None, 33, False)
        self.assertFalse(self.query_result(None, 33, True))

    def test_partial_registration_array_with_an_error_is_not_absence(self):
        """A partial empty CF array plus an error is not a successful empty lookup."""
        with self.assertRaises(RuntimeError):
            self.query_result(22, 33, True)
        self.assertFalse(self.query_result(22, None, False))

    def test_early_command_failure_quits_and_waits_before_forgetting_or_removing(self):
        """An early command error must not delete the bundle of the app it left alive."""
        scenario = Scenario([{'live': {'123': 'private app'}, 'unknown': []}, {'live': {}, 'unknown': []}])
        with self.assertRaisesRegex(ValueError, 'original command'):
            try:
                raise ValueError('original command')
            finally:
                report = scenario.finish()
        self.assertTrue(report['complete'], report)
        self.assertEqual(scenario.events, ['probe', ('quit', scenario.run.state), 'probe', 'unregister', 'query', 'remove'])

    def test_helper_without_a_ready_pid_retains_the_whole_folder_until_exit_is_known(self):
        """A failed ready answer can leave a helper even though Run.pid was never set."""
        scenario = Scenario([{'live': {'456': 'private helper'}, 'unknown': []}])
        scenario.run.pid = None
        report = scenario.finish()
        self.assertFalse(report['complete'], report)
        self.assertEqual(report['retained_work'], '/private/carry')
        self.assertIn(('quit', scenario.run.state), scenario.events)
        self.assertNotIn('unregister', scenario.events)
        self.assertNotIn('remove', scenario.events)

    def test_unknown_process_observation_cannot_authorize_removal(self):
        """An unreadable process might be this fixture's helper, so absence is unproved."""
        scenario = Scenario([{'live': {}, 'unknown': [789]}])
        self.assertFalse(scenario.finish()['complete'])
        self.assertNotIn('remove', scenario.events)

    def test_nonzero_unregister_is_accepted_only_when_exact_url_is_absent(self):
        """lsregister's already-absent error is not a failed cleanup when CF proves absence."""
        scenario = Scenario(unregister_code=1)
        report = scenario.finish()
        self.assertTrue(report['complete'], report)
        self.assertEqual(report['unregister'][0]['exit_code'], 1)
        self.assertEqual(scenario.events[-2:], ['query', 'remove'])

    def test_registered_url_is_polled_before_removal(self):
        """An accepted unregister can still leave the exact URL registered temporarily."""
        scenario = Scenario(registrations=[True, False])
        report = scenario.finish()
        self.assertTrue(report['complete'], report)
        self.assertEqual(scenario.events.count('query'), 2)
        self.assertEqual(scenario.events[-2:], ['query', 'remove'])

    def test_registration_timeout_or_unknown_retains_evidence(self):
        """An unresolved or timed-out CF lookup must retain the folder without hiding a primary error."""
        for answer in [True, None, TimeoutError('CF query timed out')]:
            with self.subTest(answer=repr(answer)):
                scenario = Scenario(registrations=[answer])
                with self.assertRaisesRegex(ValueError, 'primary'):
                    try:
                        raise ValueError('primary')
                    finally:
                        report = scenario.finish()
                self.assertFalse(report['complete'], report)
                self.assertNotIn('remove', scenario.events)

    def test_only_the_latest_client_for_one_private_state_requests_quit(self):
        """Earlier A-F clients share the state socket and must not send six quit commands."""
        scenario = Scenario([{'live': {'456': 'private helper'}, 'unknown': []}, {'live': {}, 'unknown': []}])
        older = SimpleNamespace(state=scenario.run.state, pid=99)
        self.assertTrue(scenario.finish([older, scenario.run])['complete'])
        self.assertEqual(scenario.events.count(('quit', scenario.run.state)), 1)

    def test_run_outside_private_work_is_refused_before_any_os_query(self):
        """A wrong state must never make cleanup quit the owner's real app."""
        run = SimpleNamespace(state='/owner/state', app='/private/carry/new/RectoPDF.app', pid=123)
        bundle = {'path': run.app, 'id': 'test.RectoPDF'}
        with patch.object(cleanup, 'observe', side_effect=AssertionError('must not query')):
            report = cleanup.clean_up('/private/carry', [bundle], [run])
        self.assertFalse(report['complete'], report)


if __name__ == '__main__':
    unittest.main()
