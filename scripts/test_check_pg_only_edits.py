import importlib.util
import os
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("detector", os.path.join(HERE, "check-pg-only-edits.py"))
d = importlib.util.module_from_spec(spec)
spec.loader.exec_module(d)

SPACE = "88883e1ec8261b8ac323f564e272b5be"
GEO_SPACE = "41e851aaaaaaaaaaaaaaaaaaaaaaaaaa"
EDIT = "6a1398bde58f4380adbe6209fa179a3e"
V4 = "0bd6230b6c9e48f18a1810ee5caf567e"  # a v4 id


class Conform(unittest.TestCase):
    def test_v4_passes_through(self):
        v = "0bd6230b6c9e4f18a1810ee5caf567e0"
        v = v[:12] + "4" + v[13:16] + "a" + v[17:]
        self.assertEqual(len(v), 32)
        self.assertEqual(d.conforming_id("entity", v), v)

    def test_legacy_is_hashed_with_the_card_test_vectors(self):
        # api/src/geodb/__tests__/card.test.ts
        self.assertEqual(d.conforming_id("entity", "0123456789abcdef0123456789abcdef"), "a7f79f68ecd64763867f42bfc4387c95")
        self.assertEqual(d.conforming_id("space", "00000000-0000-0000-0000-000000000001"), "e2c3812c809a4b56bd7bf07b987af9b2")

    def test_dashes_and_case_accepted(self):
        self.assertEqual(d.conforming_id("entity", "0123456789ABCDEF-0123456789abcdef"), "a7f79f68ecd64763867f42bfc4387c95")


class Missing(unittest.TestCase):
    def test_edit_pairs_absent_from_known_are_reported(self):
        pairs = [(EDIT, SPACE, V4), (EDIT, SPACE, "0123456789abcdef0123456789abcdef")]
        known = {f"{GEO_SPACE}|{d.conforming_id('entity', '0123456789abcdef0123456789abcdef')}"}
        rows = d.missing_by_edit(pairs, known, {SPACE: GEO_SPACE})
        self.assertEqual(len(rows), 1)
        self.assertEqual((rows[0]["edit"], rows[0]["space"], rows[0]["missing"], rows[0]["total"]), (EDIT, SPACE, 1, 2))

    def test_edit_fully_present_is_not_listed(self):
        pairs = [(EDIT, SPACE, V4)]
        known = {f"{GEO_SPACE}|{d.conforming_id('entity', V4)}"}
        self.assertEqual(d.missing_by_edit(pairs, known, {SPACE: GEO_SPACE}), [])

    def test_unmapped_space_is_reported_not_skipped(self):
        rows = d.missing_by_edit([(EDIT, SPACE, V4)], set(), {})
        self.assertEqual(len(rows), 1)
        self.assertTrue(rows[0]["unmapped_space"])

    def test_entity_in_other_space_does_not_count(self):
        known = {f"deadbeefdeadbeefdeadbeefdeadbeef|{d.conforming_id('entity', V4)}"}
        self.assertEqual(len(d.missing_by_edit([(EDIT, SPACE, V4)], known, {SPACE: GEO_SPACE})), 1)


class Report(unittest.TestCase):
    def test_row_carries_the_replay_arguments(self):
        edits = {EDIT: {"block": 125690, "sequence": 0, "createdAt": "2026-11-18T18:11:00", "cid": None}}
        rows = d.missing_by_edit([(EDIT, SPACE, V4)], set(), {SPACE: GEO_SPACE})
        out = d.render(rows, edits)
        for want in [SPACE, EDIT, "125690", "--block 125690", "--created-at 1795025460", "--edit " + EDIT, "<cid>"]:
            self.assertIn(want, out)

    def test_created_at_is_utc_seconds(self):
        self.assertEqual(d.unix_seconds("2026-11-18T18:11:00"), 1795025460)

    def test_version_key(self):
        self.assertEqual(d.version_key(125690, 0), 539834439434240)


class Parse(unittest.TestCase):
    def test_pairs_file(self):
        self.assertEqual(d.parse_pairs([f"{EDIT}|{SPACE}|{V4}\n", "\n"]), [(EDIT, SPACE, V4)])

    def test_bad_pairs_line_is_an_error(self):
        with self.assertRaises(ValueError):
            d.parse_pairs(["nope\n"])


if __name__ == "__main__":
    unittest.main()
