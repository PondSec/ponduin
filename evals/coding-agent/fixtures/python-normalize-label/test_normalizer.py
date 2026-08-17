import unittest

from normalizer import normalize_label


class NormalizeLabelTest(unittest.TestCase):
    def test_normalizes_surrounding_whitespace_and_case(self) -> None:
        self.assertEqual(normalize_label("  Ready FOR Review  "), "ready for review")


if __name__ == "__main__":
    unittest.main()
