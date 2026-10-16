"""Fail-closed coverage and paired median bootstrap. No third-party packages."""
import math
import random
import statistics
from collections import Counter

from suite import HIRSEL, LANGUAGES, PRIMARY, PROTOCOL, TARGETS, expected_checksum, participants, native_spec, gate_references


def bootstrap(columns, numerator, denominators):
    """One index vector per replicate, shared by numerator and all references.
    The fastest *median* is selected anew in every replicate, never frozen to
    the observed winner. The percentile is the one-sided 95% upper endpoint.
    """
    n=len(columns[numerator])
    assert n and denominators and all(len(v)==n for v in columns.values())
    observed={key:statistics.median(values) for key,values in columns.items()}
    ratio=observed[numerator]/min(observed[p] for p in denominators)
    if all(len(set(v))==1 for v in columns.values()):
        # All 10,000 paired resamples are identical, including ties. This exact
        # shortcut also keeps complete synthetic coverage tests inexpensive.
        return {'ratio':ratio,'upper95':ratio,'fastest_observed':[p for p in denominators if observed[p]==min(observed[d] for d in denominators)],
                'by_variant':{p:{'ratio':observed[numerator]/observed[p],'upper95':observed[numerator]/observed[p]} for p in denominators},
                'by_language':{lang:{'ratio':observed[numerator]/min(observed[p] for p in denominators if p.startswith(lang+'/')),'upper95':observed[numerator]/min(observed[p] for p in denominators if p.startswith(lang+'/'))} for lang in LANGUAGES if any(p.startswith(lang+'/') for p in denominators)}}
    rng=random.Random(PROTOCOL['bootstrap_seed'])
    samples=[]
    per_variant={p:[] for p in denominators}
    per_language={lang:[] for lang in LANGUAGES if any(p.startswith(lang+'/') for p in denominators)}
    for _ in range(PROTOCOL['resamples']):
        indices=[rng.randrange(n) for _ in range(n)]
        medians={p:statistics.median([columns[p][i] for i in indices]) for p in (numerator,*denominators)}
        h=medians[numerator]
        samples.append(h/min(medians[p] for p in denominators))
        for p in denominators:per_variant[p].append(h/medians[p])
        for lang,values in per_language.items():values.append(h/min(medians[p] for p in denominators if p.startswith(lang+'/')))
    def upper(v):
        v.sort();return v[math.ceil(PROTOCOL['upper_quantile']*len(v))-1]
    upper95=upper(samples)
    return {'ratio':ratio,'upper95':upper95,'fastest_observed':[p for p in denominators if observed[p]==min(observed[d] for d in denominators)],
            'by_variant':{p:{'ratio':observed[numerator]/observed[p],'upper95':upper(v)} for p,v in per_variant.items()},
            'by_language':{lang:{'ratio':observed[numerator]/min(observed[p] for p in denominators if p.startswith(lang+'/')),'upper95':upper(v)} for lang,v in per_language.items()}}


def case_samples(case,record,ps,rng):
    errors=[]
    if record.get('error'):errors.append(record['error'])
    if record.get('skip'):errors.append('case skipped: '+str(record['skip']))
    repeats=record.get('repeats')
    if not isinstance(repeats,int) or isinstance(repeats,bool) or repeats<=0:
        return {},['invalid repeat count']
    columns={p:[] for p in ps}
    for name,count in [('warmups',PROTOCOL['warmups']),('rounds',PROTOCOL['rounds'])]:
        rounds=record.get(name,[])
        if len(rounds)!=count:errors.append(f'{name}: expected {count} rounds')
        if [r.get('round') for r in rounds]!=list(range(count)):errors.append(f'{name}: duplicate or missing round IDs')
        for r in rounds:
            order=r.get('order',[]);samples=r.get('samples',[])
            expected_order=ps.copy();rng.shuffle(expected_order)
            if order!=expected_order:errors.append('order differs from preregistered randomization')
            if Counter(order)!=Counter(ps) or [s.get('participant') for s in samples]!=order:
                errors.append(f'{name}: missing/duplicate reference, variant or configuration');continue
            for s in samples:
                p=s['participant'];v=s.get('seconds')
                if s.get('skip') or s.get('excluded'):errors.append(f'{p}: skipped/excluded sample')
                if not s.get('oracle_ok') or s.get('returncode')!=0 or s.get('stdout','').encode()!=expected_checksum(case,repeats):
                    errors.append(f'{p}: output or process failure')
                if s.get('repeats')!=repeats:errors.append(f'{p}: unpaired repeat count')
                if not isinstance(v,(float,int)) or not math.isfinite(v) or v<PROTOCOL['minimum_seconds']:
                    errors.append(f'{p}: short/nonfinite timing');continue
                for state in ('before','after'):
                    meta=s.get(state,{})
                    if any(k not in meta for k in ('cpu','load','frequency_khz','governor','utc_ns')):errors.append(f'{p}: missing CPU/load/frequency metadata')
                if not isinstance(s.get('costs',{}).get('peak_rss_kib'),(int,float)):
                    errors.append(f'{p}: missing timing peak RSS')
                if name=='rounds':columns[p].append(v)
    counters=record.get('counter_runs',[])
    if Counter(c.get('participant') for c in counters)!=Counter(ps):errors.append('missing separate counter run')
    for c in counters:
        if c.get('unavailable'):
            errors.append('counter unavailable: '+c['participant']);continue
        if not c.get('oracle_ok') or c.get('returncode')!=0 or c.get('repeats')!=1 or c.get('stdout','').encode()!=expected_checksum(case,1):
            errors.append('counter correctness failed')
        if not isinstance(c.get('costs',{}).get('peak_rss_kib'),(int,float)):errors.append('missing counter peak RSS')
        counts = c.get('counters') or {}
        if c.get('participant') in HIRSEL and 'classes' in counts:
            if any(k not in counts for k in ('count_increments', 'count_decrements', 'copied_bytes',
                                             'reuse_hits', 'reuse_misses', 'peak_live_bytes',
                                             'region_high_water_bytes', 'region_pool_high_water_bytes')):
                errors.append('counter native report incomplete')
            if counts.get('arena_high_water_bytes') == 'unavailable':
                errors.append('counter arena unavailable: '+c['participant'])
        elif any(k not in counts for k in ('allocation_requests','copied_bytes','count_operations','reuse_hits','reuse_misses','arena_peak_reserved_bytes')):
            errors.append('missing allocation/RC/copy/reuse/arena counters')
    if any(len(v)!=PROTOCOL['rounds'] for v in columns.values()):errors.append('missing paired timing column')
    return columns,sorted(set(errors))


def analyse(manifest,datasets):
    obligations=[];inconclusive=[];failures=[];rows=[];seen=set();metadata=[]
    wanted={c['id'] for c in manifest['cases']}
    for data in datasets:
        target=data.get('target')
        if target not in TARGETS:
            obligations.append(f'unknown/missing target: {target}');continue
        if target in seen:
            obligations.append(f'duplicate target dataset: {target} (reruns must be analysed independently)');continue
        seen.add(target)
        if data.get('schema')!=1 or data.get('protocol')!=PROTOCOL:obligations.append(f'{target}: protocol mismatch')
        if data.get('manifest')!=manifest:obligations.append(f'{target}: source/fixture manifest mismatch')
        builds=data.get('build',{}).get('executables',{})
        if data.get('build',{}).get('source_sha256')!=manifest['source_sha256']:obligations.append(f'{target}: build source hashes missing/mismatched')
        for p in {p for c in manifest['cases'] for p in participants(manifest,c)}:
            for suffix in ('','-stats'):
                b=builds.get(p+suffix,{})
                if any(k not in b for k in ('sha256','text_bytes','compile_seconds','command')):obligations.append(f'{target}: missing binary/text/compile evidence for {p}{suffix}')
        native=data.get('hirsel_build')
        if not native or set(native)!=set(HIRSEL):obligations.append(f'{target}: missing Hirsel configurations (N/F/J dependency)')
        if native:
            for case in manifest['cases']:
                for p in HIRSEL:
                    spec=native_spec(native,p,case) or {}
                    if any(k not in spec for k in ('source_sha256','binary_sha256','counter_binary_sha256','text_bytes','compile_seconds','command','counter_command')):
                        obligations.append(f"{target}/{case['id']}: incomplete native build evidence for {p}")
        machine=data.get('machine')
        if not machine or not machine.get('dedicated') or machine.get('target')!=target or any(k not in machine for k in ('cpu','governor','cpu_feature_baseline','os','microcode')):
            obligations.append(f'{target}: dedicated pinned machine declaration missing')
        evidence=data.get('correctness',{})
        if not evidence.get('passed') or evidence.get('failures'):obligations.append(f'{target}: reference correctness failed/missing')
        for case in manifest['cases']:
            cs=evidence.get('input_oracles',{}).get(case['id'],{})
            if cs!={'input':case['input_sha256'],'oracle':case['oracle_sha256']}:obligations.append(f"{target}/{case['id']}: missing input/byte correctness evidence")
            tested=evidence.get('participants_by_case',{}).get(case['id'],[])
            if Counter(tested)!=Counter(participants(manifest,case,True)):obligations.append(f"{target}/{case['id']}: missing correctness participant")
        records=data.get('cases',[])
        ids=[c.get('id') for c in records]
        if set(ids)!=wanted or len(ids)!=len(wanted):obligations.append(f'{target}: missing/duplicate/unknown required input')
        metadata.append({'target':target,'build_costs':builds,'hirsel_costs':native,'hardware':data.get('hardware'),
                         'machine':machine,'manifest_sha256':data.get('manifest_sha256')})
        rng=random.Random(PROTOCOL['order_seed'])
        for case in manifest['cases']:
            matches=[c for c in records if c.get('id')==case['id']]
            if len(matches)!=1:continue
            record=matches[0];refs=participants(manifest,case)
            actual_h=[p for p in HIRSEL if any(s.get('participant')==p for r in record.get('rounds',[]) for s in r.get('samples',[]))]
            ps=participants(manifest,case,native)
            columns,errors=case_samples(case,record,ps,rng)
            if errors:
                obligations.extend(f"{target}/{case['id']}: {e}" for e in errors)
                # Missing telemetry does not erase valid timing evidence, and
                # cannot pass the gate. Timing/protocol errors still refuse ratios.
                telemetry=('counter ', 'missing separate counter', 'missing counter peak RSS', 'missing allocation/RC/copy/reuse/arena counters')
                if any(not e.startswith(telemetry) for e in errors):continue
            if machine:
                for r in record['warmups']+record['rounds']:
                    for s in r['samples']:
                        for state in ('before','after'):
                            if s[state]['cpu']!=machine.get('cpu') or s[state]['governor']!=machine.get('governor'):
                                obligations.append(f"{target}/{case['id']}: machine CPU/governor changed")
            medians={p:statistics.median(v) for p,v in columns.items()}
            row={'case':case['id'],'program':case['program'],'target':target,'medians_seconds':medians,
                 'repeats':record['repeats'],'costs':{p:{'peak_rss_kib':max(s['costs']['peak_rss_kib'] for r in record['rounds'] for s in r['samples'] if s['participant']==p)} for p in ps},
                 'counters':{c['participant']:c.get('counters',{}) for c in record.get('counter_runs',[])},'ratios':{}}
            for p in actual_h:
                try:denominators=gate_references(manifest,case,p,native)
                except ValueError as error:
                    obligations.append(f"{target}/{case['id']}/{p}: {error}");continue
                result=bootstrap(columns,p,denominators)
                result['gate_references']=denominators
                if denominators!=refs:
                    # Retain every pairwise column for informational regions
                    # tables; only the eligible set controls the gate statistic.
                    result['by_variant']=bootstrap(columns,p,refs)['by_variant']
                row['ratios'][p]=result
                if p in PRIMARY and result['upper95']>PROTOCOL['gate']:failures.append({'case':case['id'],'target':target,'configuration':p,'upper95':result['upper95']})
                # Preregistered ten-percent relative bound inflation. The data
                # remain in the report; no outliers or rounds are deleted.
                if p in PRIMARY and result['upper95']/result['ratio']>PROTOCOL['maximum_upper_inflation']:inconclusive.append(f"{target}/{case['id']}/{p}: noisy upper bound")
            for p in PRIMARY:
                if p not in actual_h:obligations.append(f"{target}/{case['id']}: missing primary partition {p}")
            rows.append(row)
    for target in TARGETS:
        if target not in seen:obligations.append(f'missing target: {target}')
    # Target/variant aggregates are descriptive. A geometric mean cannot hide
    # one required failing case and never participates in the gate decision.
    aggregates=[]
    for target in TARGETS:
        for config in HIRSEL:
            valid=[r['ratios'][config] for r in rows if r['target']==target and config in r['ratios']]
            if not valid:continue
            aggregates.append({'target':target,'configuration':config,'cases':len(valid),
                'geometric_mean_ratio':math.exp(statistics.mean(math.log(r['ratio']) for r in valid)),
                'worst_upper95':max(r['upper95'] for r in valid),
                'by_variant':{p:{'geometric_mean_ratio':math.exp(statistics.mean(math.log(r['by_variant'][p]['ratio']) for r in valid if p in r['by_variant'])),
                                 'worst_upper95':max(r['by_variant'][p]['upper95'] for r in valid if p in r['by_variant'])} for p in sorted({p for r in valid for p in r['by_variant']})}})
    status='incomplete' if obligations else 'inconclusive' if inconclusive else 'fail' if failures else 'pass'
    return {'schema':1,'status':status,'gate':PROTOCOL['gate'],'bootstrap':{'seed':PROTOCOL['bootstrap_seed'],'resamples':PROTOCOL['resamples'],'paired':True,'minimum_recomputed':True},
            'open':sorted(set(obligations)),'inconclusive':sorted(set(inconclusive)),'failed_cases':failures,
            'cases':rows,'targets':aggregates,'costs_separate_from_ratios':metadata}
