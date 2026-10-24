"""Finite original recording namespace and strict child completion contract."""
BACKENDS = ('llvm', 'cranelift')
WIDTHS = (1, 2, 4)
HOST_MODES = ('success', 'domain', 'cancel')


def inventory(cases):
    names = [f'{case["id"]}-{backend}-{width}'
             for case in cases for backend in BACKENDS for width in WIDTHS]
    names += [f'host-{mode}-{backend}-{width}'
              for mode in HOST_MODES for backend in BACKENDS for width in WIDTHS]
    assert len(names) == len(set(names)), 'duplicate cell'
    return tuple(names)


def select_cells(cases, requested):
    valid = inventory(cases)
    selected = tuple(requested)
    assert selected, 'empty cell selection'
    assert len(selected) == len(set(selected)), 'duplicate selection'
    assert set(selected) <= set(valid), ('unknown cell', set(selected) - set(valid))
    return selected


def required_receipts(selected):
    assert selected, 'empty cell selection'
    return {*(backend + '-build' for backend in BACKENDS), *selected,
            *(cell + '-replay-' + backend for cell in selected for backend in BACKENDS)}


def check_receipts(selected, receipts):
    actual = [row['id'] for row in receipts]
    assert len(actual) == len(set(actual)), 'duplicate receipt'
    assert set(actual) == required_receipts(selected), ('incomplete dispatch', actual)
    assert all(row.get('passed') is True for row in receipts), ('failed child', receipts)
    for row in receipts:
        if row['id'].endswith('-build'):
            assert row['status'] == 0, row
