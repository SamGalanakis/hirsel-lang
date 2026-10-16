import copy
import json
import random
import sys
import unittest
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from analysis import analyse, bootstrap
from suite import controlled_environment, ROOT, HIRSEL, PRIMARY, PROTOCOL, TARGETS, expected_checksum, participants


def fixture():
    m=json.loads((ROOT/'manifest.json').read_text())
    m['cases']=[m['cases'][0],next(c for c in m['cases'] if c['id']=='batch-validation-small')]
    datasets=[]
    for target in TARGETS:
        def s(p,c,v=0.8):
            meta={'cpu':24,'load':[0,0,0],'frequency_khz':'3000000','governor':'performance','utc_ns':1}
            return {'participant':p,'seconds':v,'repeats':1,'oracle_ok':True,'returncode':0,
                    'stdout':expected_checksum(c,1).decode(),'before':meta,'after':meta,'costs':{'peak_rss_kib':100},
                    'counters':{'allocation_requests':1,'copied_bytes':0,'count_operations':0,'reuse_hits':0,'reuse_misses':0,'arena_peak_reserved_bytes':0}}
        builds={p+suffix:{'sha256':'0'*64,'text_bytes':1,'compile_seconds':0.1,'command':['fixture']} for p in participants(m,m['cases'][0]) for suffix in ('','-stats')}
        native={p:{'source_sha256':'0'*64,'binary_sha256':'1'*64,'counter_binary_sha256':'2'*64,'text_bytes':1,'compile_seconds':0.1,'command':['fixture'],'counter_command':['fixture']} for p in HIRSEL}
        data={'schema':1,'target':target,'protocol':PROTOCOL,'manifest':m,
              'build':{'source_sha256':m['source_sha256'],'executables':builds},'hirsel_build':native,
              'machine':{'dedicated':True,'target':target,'cpu':24,'governor':'performance','cpu_feature_baseline':'baseline','os':'fixture','microcode':'fixture'},
              'correctness':{'passed':True,'failures':[],'input_oracles':{c['id']:{'input':c['input_sha256'],'oracle':c['oracle_sha256']} for c in m['cases']},'participants_by_case':{c['id']:participants(m,c,True) for c in m['cases']}},'cases':[]}
        rng=random.Random(PROTOCOL['order_seed'])
        for c in m['cases']:
            ps=participants(m,c,True)
            record={'id':c['id'],'repeats':1,'warmups':[],'rounds':[],'counter_runs':[s(p,c) for p in ps]}
            for name,n in [('warmups',5),('rounds',30)]:
                for i in range(n):
                    order=ps.copy();rng.shuffle(order)
                    record[name].append({'round':i,'order':order,'samples':[s(p,c,1.0 if p in HIRSEL else 0.8) for p in order]})
            data['cases'].append(record)
        datasets.append(data)
    return m,datasets


def set_time(data,case_index,predicate,value):
    for target in data:
        for name in ('warmups','rounds'):
            for r in target['cases'][case_index][name]:
                for s in r['samples']:
                    if predicate(s['participant']):s['seconds']=value

class AnalysisTests(unittest.TestCase):
    def test_ambient_allocator_and_profile_options_cannot_change_pins(self):
        from unittest.mock import patch
        import os
        values={'LD_PRELOAD':'other.so','MIMALLOC_VERBOSE':'1','CARGO_PROFILE_RELEASE_OPT_LEVEL':'0','CARGO_ENCODED_RUSTFLAGS':'-Ctarget-cpu=native','PATH':'kept'}
        with patch.dict(os.environ,values,clear=True):
            env=controlled_environment({'RUSTFLAGS':'baseline'})
        self.assertEqual(env,{'PATH':'kept','RUSTFLAGS':'baseline'})
    def test_complete_constant_125_passes(self):
        m,d=fixture();self.assertEqual(analyse(m,d)['status'],'pass')
    def test_one_126_case_fails_even_when_geomean_passes(self):
        m,d=fixture();set_time(d,0,lambda p:p in PRIMARY,1.008);set_time(d,1,lambda p:p in PRIMARY,0.4)
        r=analyse(m,d);self.assertEqual(r['status'],'fail');self.assertTrue(r['failed_cases'])
        self.assertLess(r['targets'][0]['geometric_mean_ratio'],1.25)
    def test_fast_arena_and_mimalloc_each_control_denominator(self):
        for variant in ('rust/arena','rust/mimalloc'):
            with self.subTest(variant=variant):
                m,d=fixture();set_time(d,1,lambda p:p not in HIRSEL,1.0);set_time(d,1,lambda p:p in PRIMARY,1.1);set_time(d,1,lambda p:p==variant,0.8)
                r=analyse(m,d);self.assertEqual(r['status'],'fail');self.assertLess(r['cases'][1]['ratios'][PRIMARY[0]]['by_variant']['c/system']['upper95'],1.25)
    def test_binarytrees_arena_and_heap_have_separate_c_denominators(self):
        m,d=fixture()
        for data in d:data['hirsel_build'][PRIMARY[1]]['binarytrees_storage']='arena'
        set_time(d,0,lambda p:p not in HIRSEL,1.0)
        set_time(d,0,lambda p:p in PRIMARY,1.1)
        set_time(d,0,lambda p:p=='c/arena',0.8)
        set_time(d,0,lambda p:p=='rust/arena',0.5)
        result=analyse(m,d)
        self.assertEqual(result['status'],'fail')
        self.assertEqual({f['configuration'] for f in result['failed_cases']},{PRIMARY[1]})
        for row in (r for r in result['cases'] if r['program']=='binarytrees'):
            heap,arena=(row['ratios'][p] for p in PRIMARY)
            self.assertAlmostEqual(heap['ratio'],1.1)
            self.assertEqual(heap['gate_references'],['c/system','c/mimalloc'])
            self.assertAlmostEqual(heap['by_variant']['rust/arena']['ratio'],2.2)
            self.assertEqual(arena['fastest_observed'],['c/arena'])
            self.assertAlmostEqual(arena['ratio'],1.375)
    def test_binarytrees_heap_selects_the_fastest_malloc_c(self):
        m,d=fixture()
        set_time(d,0,lambda p:p=='c/system',0.9)
        set_time(d,0,lambda p:p=='c/mimalloc',0.8)
        set_time(d,0,lambda p:p in ('c/arena','rust/arena'),0.2)
        result=analyse(m,d)
        for row in (r for r in result['cases'] if r['program']=='binarytrees'):
            self.assertEqual(row['ratios'][PRIMARY[0]]['fastest_observed'],['c/mimalloc'])
            self.assertAlmostEqual(row['ratios'][PRIMARY[0]]['ratio'],1.25)
    def test_invalid_binarytrees_storage_cannot_pass(self):
        m,d=fixture();d[0]['hirsel_build'][PRIMARY[0]]['binarytrees_storage']='unknown'
        result=analyse(m,d)
        self.assertEqual(result['status'],'incomplete')
        self.assertTrue(any('invalid binarytrees_storage' in item for item in result['open']))
    def test_archived_binarytrees_keeps_its_embedded_baseline_contract(self):
        m,d=fixture();del m['binarytrees_gate']
        set_time(d,0,lambda p:p=='rust/arena',0.4)
        result=analyse(m,d)
        self.assertEqual(result['status'],'fail')
        self.assertEqual(result['cases'][0]['ratios'][PRIMARY[0]]['fastest_observed'],['rust/arena'])
    def test_missing_target(self):
        m,d=fixture();self.assertEqual(analyse(m,d[:1])['status'],'incomplete')
    def test_missing_language(self):
        m,d=fixture();d[0]['cases'][0]['rounds'][0]['samples']=[s for s in d[0]['cases'][0]['rounds'][0]['samples'] if not s['participant'].startswith('c/')]
        self.assertEqual(analyse(m,d)['status'],'incomplete')
    def test_missing_variant(self):
        m,d=fixture();d[0]['cases'][0]['rounds'][0]['samples']=[s for s in d[0]['cases'][0]['rounds'][0]['samples'] if s['participant']!='rust/arena']
        self.assertEqual(analyse(m,d)['status'],'incomplete')
    def test_explicit_skip(self):
        m,d=fixture();d[0]['cases'][0]['skip']='unsupported';self.assertEqual(analyse(m,d)['status'],'incomplete')
    def test_deleted_case(self):
        m,d=fixture();d[0]['cases'].pop();self.assertEqual(analyse(m,d)['status'],'incomplete')
    def test_missing_primary_partition(self):
        m,d=fixture();d[0]['hirsel_build'].pop(PRIMARY[1]);self.assertEqual(analyse(m,d)['status'],'incomplete')
    def test_missing_counter_run(self):
        m,d=fixture();d[0]['cases'][0]['counter_runs'].pop();self.assertEqual(analyse(m,d)['status'],'incomplete')
    def test_output_mismatch(self):
        m,d=fixture();d[0]['cases'][0]['rounds'][0]['samples'][0]['stdout']='bad';self.assertEqual(analyse(m,d)['status'],'incomplete')
    def test_short_sample(self):
        m,d=fixture();d[0]['cases'][0]['rounds'][0]['samples'][0]['seconds']=0.249;self.assertEqual(analyse(m,d)['status'],'incomplete')
    def test_duplicate_target_cannot_choose_better_rerun(self):
        m,d=fixture();self.assertEqual(analyse(m,d+[copy.deepcopy(d[0])])['status'],'incomplete')
    def test_missing_machine_certification(self):
        m,d=fixture();d[0]['machine']=None;self.assertEqual(analyse(m,d)['status'],'incomplete')
    def test_missing_byte_oracle_evidence(self):
        m,d=fixture();d[0]['correctness']['input_oracles'].pop(m['cases'][0]['id']);self.assertEqual(analyse(m,d)['status'],'incomplete')
    def test_changed_fixture_manifest(self):
        m,d=fixture();d[0]['manifest']=copy.deepcopy(m);d[0]['manifest']['cases'][0]['checksum']='bad';self.assertEqual(analyse(m,d)['status'],'incomplete')
    def test_missing_informational_config_still_incomplete(self):
        m,d=fixture()
        for key in ('warmups','rounds'):
            for row in d[0]['cases'][0][key]:
                row['order'].remove('regions-off-system')
                row['samples']=[s for s in row['samples'] if s['participant']!='regions-off-system']
        d[0]['cases'][0]['counter_runs']=[s for s in d[0]['cases'][0]['counter_runs'] if s['participant']!='regions-off-system']
        self.assertEqual(analyse(m,d)['status'],'incomplete')
    def test_missing_rss_is_incomplete(self):
        m,d=fixture();d[0]['cases'][0]['counter_runs'][0].pop('costs')
        self.assertEqual(analyse(m,d)['status'],'incomplete')
    def test_nonrandom_order_is_incomplete(self):
        m,d=fixture()
        for key in ('warmups','rounds'):
            for row in d[0]['cases'][0][key]:
                row['samples'].sort(key=lambda s:s['participant'])
                row['order']=[s['participant'] for s in row['samples']]
        self.assertEqual(analyse(m,d)['status'],'incomplete')
    def test_bootstrap_reselects_minimum(self):
        columns={'H':[1.0]*30,'c/system':[1.0]*30,'rust/arena':[0.9,1.1]*15}
        result=bootstrap(columns,'H',['c/system','rust/arena'])
        self.assertGreater(result['upper95'],1.10)
        self.assertEqual(result,bootstrap(columns,'H',['c/system','rust/arena']))
    def test_paired_samples_preserve_common_noise(self):
        x=[1+i/100 for i in range(30)]
        r=bootstrap({'H':[v*1.2 for v in x],'c/system':x},'H',['c/system'])
        self.assertAlmostEqual(r['upper95'],1.2)
    def test_nonfinite_sample(self):
        m,d=fixture();d[0]['cases'][0]['rounds'][0]['samples'][0]['seconds']=float('nan');self.assertEqual(analyse(m,d)['status'],'incomplete')
    def test_arenas_only_for_registered_allocation_cases(self):
        m,d=fixture();shared=next(c for c in json.loads((ROOT/'manifest.json').read_text())['cases'] if c['case']=='persistent-list-map/shared')
        self.assertEqual(len(participants(m,shared)),4)
        self.assertEqual(len(participants(m,m['cases'][0])),6)

class NativeEvidenceTests(unittest.TestCase):
    def test_native_classes_keep_timing_but_unavailable_arena_cannot_pass(self):
        from test_measurement import REPORT
        from suite import parse_measurement
        m,d=fixture()
        for data in d:
            for record in data['cases']:
                for counter in record['counter_runs']:
                    if counter['participant'] in HIRSEL:
                        counter['counters'] = parse_measurement(REPORT)
        result = analyse(m,d)
        self.assertEqual(result['status'], 'incomplete')
        self.assertEqual(len(result['cases']), 4)
        self.assertTrue(any('counter arena unavailable' in item for item in result['open']))
        self.assertFalse(any('missing allocation/RC' in item for item in result['open']))

    def test_missing_telemetry_retains_ratios_but_never_passes(self):
        m,d=fixture()
        for data in d:
            for record in data['cases']:
                record['counter_runs'] = [
                    dict(participant=row['participant'],unavailable='pending')
                    if row['participant'] in HIRSEL else row
                    for row in record['counter_runs']
                ]
        r=analyse(m,d)
        self.assertEqual(r['status'],'incomplete')
        self.assertEqual(len(r['cases']),4)
        self.assertIn(PRIMARY[0],r['cases'][0]['ratios'])
        self.assertEqual(r['cases'][0]['counters'][PRIMARY[0]],{})

    def test_archived_manifest_keeps_contract_validation(self):
        from suite import load_manifest
        m=json.loads((ROOT/'manifest.json').read_text())
        self.assertEqual(load_manifest(m,check_files=False),m)
        broken=copy.deepcopy(m);broken['required_targets'].pop()
        with self.assertRaises(AssertionError):load_manifest(broken,check_files=False)

    def test_native_per_case_commands_use_the_selected_executable(self):
        from suite import command
        m,d=fixture();case=m['cases'][0]
        specs={PRIMARY[0]:{'cases':{case['case']:{'command':['/native/tree','{case}','{input}','{rounds}','{emit}']}}}}
        self.assertEqual(command(PRIMARY[0],case,3,{},specs),['/native/tree',case['case'],case['input'],'3'])
        self.assertEqual(participants(m,case,specs),participants(m,case)+[PRIMARY[0]])
        self.assertEqual(participants(m,m['cases'][1],specs),participants(m,m['cases'][1]))

if __name__=='__main__':unittest.main()
