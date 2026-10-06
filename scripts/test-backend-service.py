"""Check packaged backend identity and delayed listener readiness."""
import importlib.util
import json
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("backend_service", Path(__file__).with_name("backend-service.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class BackendServiceTests(unittest.TestCase):
    def setUp(self):
        self.payload = Path("/package/ribbon-payload-test.dylib")
        self.user = SimpleNamespace(pw_uid=501, pw_gid=20)
        self.backend = dict(ok=True, version=2, build=self.payload.name, uid=501, pid=123)

    def test_identity_rejects_old_build_wrong_dock_user_and_protocol(self):
        self.assertTrue(module.expected_backend(self.backend, self.payload, 501, 123))
        for key, value in dict(ok=False, version=1, build="old", uid=502, pid=124).items():
            self.assertFalse(module.expected_backend(self.backend | {key: value}, self.payload, 501, 123))
        self.assertFalse(module.expected_backend(None, self.payload, 501, 123))

    def test_listener_can_become_ready_after_loader_returns(self):
        responses = [SimpleNamespace(stdout=json.dumps(dict(backend=None))),
                     SimpleNamespace(stdout=json.dumps(dict(backend=self.backend)))]
        with patch.object(module.subprocess, "run", side_effect=responses) as run, patch.object(module.time, "sleep"):
            self.assertEqual(module.wait_for_backend(["probe"], self.user, self.payload, 123), self.backend)
            self.assertEqual(run.call_count, 2)
            self.assertEqual(run.call_args.kwargs["user"], 501)

    def test_mismatch_reports_expected_and_actual_identity(self):
        result = SimpleNamespace(stdout=json.dumps(dict(backend=self.backend | dict(build="old"))))
        with patch.object(module.subprocess, "run", return_value=result):
            with self.assertRaisesRegex(RuntimeError, "expected ribbon-payload-test.*received.*old"):
                module.wait_for_backend(["probe"], self.user, self.payload, 123, timeout=0)


if __name__ == "__main__":
    unittest.main()
