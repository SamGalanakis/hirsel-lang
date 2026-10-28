fn check_cpu_lifecycle(path: &std::path::Path, width: usize, jitter: bool, domain_count: usize) {
    use std::{
        collections::{BTreeMap, BTreeSet},
        fs,
    };
    let text = fs::read_to_string(path).unwrap();
    let mut live = BTreeMap::new();
    let mut domains = BTreeMap::<u64, BTreeSet<usize>>::new();
    let mut retired = BTreeMap::<u64, BTreeSet<usize>>::new();
    let mut shutdown = BTreeSet::new();
    let mut callers = BTreeMap::new();
    let mut perturbations = 0;
    let mut runs = 0;
    for line in text.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        assert_eq!(fields.len(), 4, "{line}");
        let domain: u64 = fields[1].parse().unwrap();
        let owner: usize = fields[2].parse().unwrap();
        let value: usize = fields[3].parse().unwrap();
        match fields[0] {
            "caller" => {
                assert!(domain > 0);
                assert_eq!(value, 0);
                assert!(callers.insert(domain, owner).is_none());
                assert!(live.insert(owner, (domain, false, false)).is_none());
                assert!(domains.entry(domain).or_default().insert(value));
            }
            "start" => {
                assert!(domain > 0 && (1..width).contains(&value));
                assert!(live.insert(owner, (domain, false, false)).is_none());
                assert!(domains.entry(domain).or_default().insert(value));
            }
            "park" => {
                let (_, exited, parked) = live.get_mut(&owner).unwrap();
                assert!(!*exited && !*parked, "{line}");
                *parked = true;
            }
            "wake" => {
                let (_, exited, parked) = live.get_mut(&owner).unwrap();
                assert!(!*exited && *parked, "{line}");
                *parked = false;
            }
            "run" => {
                let (_, exited, parked) = live[&owner];
                assert!(!exited && !parked, "{line}");
                runs += 1;
            }
            "exit" => {
                let (_, exited, parked) = live.get_mut(&owner).unwrap();
                assert!(!*exited && !*parked, "{line}");
                *exited = true;
            }
            "retire" => {
                assert_eq!(live.remove(&owner), Some((domain, true, false)), "{line}");
                assert!(retired.entry(domain).or_default().insert(owner));
            }
            "shutdown" => {
                assert!(shutdown.insert(domain));
                assert_eq!(value, width);
                assert_eq!(domains[&domain].len(), width);
                assert_eq!(retired.get(&domain).map_or(0, BTreeSet::len), width - 1);
                let caller = callers[&domain];
                assert_eq!(live.remove(&caller), Some((domain, false, false)));
                assert!(live.values().all(|(active, _, _)| *active != domain));
            }
            "jitter" => {
                assert!(jitter && domains.contains_key(&domain) && !shutdown.contains(&domain));
                assert!(owner < width && (1..=4).contains(&value));
                perturbations += 1;
            }
            other => panic!("unexpected CPU lifecycle event {other}: {line}"),
        }
    }
    assert!(live.is_empty(), "CPU workers survived shutdown: {text}");
    assert_eq!(
        domains.len(),
        domain_count,
        "each operation constructs a fresh domain"
    );
    assert_eq!(shutdown.len(), domain_count);
    assert!(runs > 0);
    assert_eq!(
        perturbations > 0,
        jitter,
        "mapper admission or checkpoints must execute jitter"
    );
}
