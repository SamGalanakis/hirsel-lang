import sys, json, subprocess, os
from pathlib import Path
root=Path(__file__).resolve().parents[2]
sys.path.insert(0,str(root/'bench/native'))
from suite import native_environment, buck2_build
out=Path(sys.argv[1]).resolve();out.mkdir(parents=True,exist_ok=True)
env=native_environment({'HIRSEL_BENCH_MEASUREMENT':'1'})
(out/'environment.json').write_text(json.dumps(env))
helper=Path(buck2_build('//:hirsel-native-build-bin-hirsel-native-build')['hirsel-native-build-bin-hirsel-native-build'])
programs=['examples/tasks/hirsel.json', 'conformance/CONC/CONC-2-task-all-ok-run.hir', 'conformance/MEM/MEM-11-tour-11-shared-config-run.hir', 'conformance/CONC/CONC-2-task-select-fast-slow-run.hir', 'conformance/CONC/CONC-3-check-canceled-run.hir', 'conformance/CLO/CLO-3-child-mutex-capture-run.hir', 'bench/native/programs/controls/resource-drops.hir']
for source in programs:
 name='tasks-app' if source.endswith('hirsel.json') else Path(source).stem
 dest=out/name;dest.mkdir(exist_ok=True)
 with (dest/'build.stdout').open('w') as stdout,(dest/'build.stderr').open('w') as stderr:
  result=subprocess.run([str(root/'tools/capped'),'--timeout','900',str(helper),str(root/source),str(dest/'program'),'regions-on-whole'],env=env,cwd=root,stdout=stdout,stderr=stderr)
 print(name, result.returncode,flush=True)
