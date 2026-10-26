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


class LineageAdapter(unittest.TestCase):
    def view(self):
        vector = copy.deepcopy(DATA['vectors'][0])
        vector['lineage'] = dict(complete=True, missing=[], events=vector.pop('events'),
                                witnessed=[], coverage=dict(admission='witnessed',
                                request_credits='witnessed', upstream_credits='witnessed'))
        return vector

    def test_complete_internal_join_reaches_all_invariants(self):
        view = self.view()
        self.assertEqual(judge_recording(view, view['replay']['expected'])['semantic_verdict'], 'pass')

    def test_partial_join_never_reaches_checker(self):
        for field, value in [('complete', False), ('missing', ['RPC timing mapping missing']),
                             ('coverage', {}), ('events', None)]:
            with self.subTest(field=field):
                view = self.view()
                view['lineage'][field] = value
                verdict = judge_recording(view, view['replay']['expected'])
                self.assertEqual(verdict['semantic_verdict'], 'unproven: witness missing')

    def test_supported_ownership_does_not_excuse_bad_backoff(self):
        view = self.view()
        event = next(e for e in view['lineage']['events'] if e['op'] == 'backoff')
        event['until'] += 1
        self.assertEqual(judge_recording(view, view['replay']['expected'])['semantic_verdict'], 'fail: backoff')

    def test_recorded_bytes_must_match_checked_report(self):
        view = self.view()
        wrong = dict(view['replay']['expected'], stdout='forged')
        self.assertEqual(judge_recording(view, wrong)['semantic_verdict'], 'fail: report')

    def test_missing_admission_mapping_does_not_pass(self):
        view = self.view()
        del view['config']
        self.assertEqual(judge_recording(view, view['replay']['expected'])['semantic_verdict'],
                         'unproven: witness missing')


if __name__ == '__main__':
    unittest.main()
