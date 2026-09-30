import importlib.util
from pathlib import Path
import unittest

path = Path(__file__).parents[1] / "evalproof" / "adapter.py"
spec = importlib.util.spec_from_file_location("adapter", path)
adapter = importlib.util.module_from_spec(spec)
spec.loader.exec_module(adapter)


class NormalizationTests(unittest.TestCase):
    def test_numeric_score_cannot_silently_become_pass(self):
        with self.assertRaises(ValueError):
            adapter.normalize(0.1)

    def test_errors_are_not_rejections(self):
        self.assertEqual(adapter.normalize({"pass": False, "error": "timeout"})["verdict"], "error")

    def test_conflicting_verdict_is_invalid(self):
        with self.assertRaises(ValueError):
            adapter.normalize({"pass": True, "verdict": "reject"})

    def test_invalid_cost_is_rejected(self):
        for cost in (-1, float("nan"), True, "100"):
            with self.assertRaises(ValueError):
                adapter.normalize({"verdict": "accept", "cost_microusd": cost})


if __name__ == "__main__":
    unittest.main()
