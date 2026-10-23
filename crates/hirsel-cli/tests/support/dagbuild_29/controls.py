"""Refusal controls mutate a copy's bound inputs, never the archival tape."""
def controls(root,out,binaries,run,hirsel):
    import hashlib,json,shutil
    from pathlib import Path
    rows=[]
    # Preserve original no-effect paths: each tested tape is outside absent mounts.
    for backend,binary in binaries.items():
        source=Path(__import__('os').environ['DAGBUILD_29_CONTROL_TAPE'])
        original=source.read_bytes()
        for kind,data in [('truncated',original[:-1]),('corrupt',original[:64]+bytes([original[64]^1])+original[65:])]:
            path=out/(backend+'-'+kind+'.tape');path.write_bytes(data)
            p=run([str(binary),'--hirsel-replay',str(path)],backend+'-control-'+kind)
            assert p.returncode==3 and not p.stdout,(backend,kind,p)
            rows.append(dict(engine=backend,control=kind,exit=p.returncode,passed=True))
        assert source.read_bytes()==original
    # Source/config changes must refuse the original semantic binding before entry.
    candidate=out/'changed-source';candidate.mkdir()
    for name in ['main','manifest','graph','fingerprint','execute','report']:
        shutil.copy2(root/(name+'.hir'),candidate/(name+'.hir'))
    manifest=json.loads((root/'hirsel.json').read_text())
    (candidate/'hirsel.json').write_text(json.dumps(manifest))
    file=candidate/'main.hir';file.write_text(file.read_text().replace('30000000000u64','29999999999u64'))
    p=run(['env','HIRSEL_JOBS=1',str(hirsel),'run',str(candidate/'hirsel.json'),'--replay',str(source),'--max-steps','1000000'],'changed-config-control')
    assert p.returncode==3 and not p.stdout,p
    rows.append(dict(engine='reference',control='changed-program-config',exit=p.returncode,passed=True))
    (out/'control-receipts.json').write_text(json.dumps(rows,indent=2))
    return rows
