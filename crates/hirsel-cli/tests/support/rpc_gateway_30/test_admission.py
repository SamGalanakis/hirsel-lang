"""Regular regression cases for the admission contract and its own test oracle."""
import copy
import json
from pathlib import Path
import unittest

from admission import INVARIANTS, Violation, check, check_catalog, judge_recording

DATA = json.loads(Path(__file__).with_name('admission_vectors.json').read_text())


class Vectors(unittest.TestCase):
    def test_catalog(self):
        self.assertEqual(check_catalog(DATA), dict(positive=27, negative=29, invariants=17))


def vector_test(vector):
    def test(self):
        if vector['expected'] == 'accept':
            self.assertLessEqual(check(vector), INVARIANTS)
        else:
            with self.assertRaises(Violation) as error:
                check(vector)
            self.assertEqual(str(error.exception), vector['expected'])
    return test


for vector in DATA['vectors']:
    setattr(Vectors, 'test_' + vector['name'].replace('-', '_'), vector_test(vector))


class Sabotage(unittest.TestCase):
    def test_wrong_positive_expectation(self):
        data = copy.deepcopy(DATA)
        data['vectors'][0]['expected'] = 'deadline'
        with self.assertRaisesRegex(ValueError, 'expected deadline, got accept'):
            check_catalog(data)

    def test_wrong_negative_expectation(self):
        data = copy.deepcopy(DATA)
        vector = next(v for v in data['vectors'] if v['name'] == 'duplicate-request')
        vector['expected'] = 'accept'
        with self.assertRaisesRegex(ValueError, 'expected accept, got identity'):
            check_catalog(data)

    def test_missing_negative_coverage(self):
        data = copy.deepcopy(DATA)
        data['vectors'] = [v for v in data['vectors'] if v['expected'] != 'join']
        with self.assertRaisesRegex(ValueError, 'negative coverage'):
            check_catalog(data)


class MissingWitness(unittest.TestCase):
    def test_output_and_complete_view_cannot_prove_admission(self):
        for view in [None, dict(complete=True, events=[]),
                     dict(valid=True, active_count=16, retry_allowed=True)]:
            with self.subTest(view=view):
                output = DATA['vectors'][0]['replay']['expected']
                verdict = judge_recording(view, output)
                self.assertEqual(verdict['semantic_verdict'], 'unproven: witness missing')
                self.assertEqual(verdict['recorded_output'], output)
                self.assertEqual(verdict['coverage']['request_credits'], 'unproven')


if __name__ == '__main__':
    unittest.main()
