use super::*;

#[test]
fn component_budget_uses_utf8_bytes_and_keeps_original_path() {
    let budget = NameBudget::from_maximum(255).unwrap();
    let ascii = "a".repeat(239);
    assert_eq!(budget.path(&ascii).unwrap(), ascii);
    let unicode = format!("{}ab", "界".repeat(79));
    assert_eq!(budget.path(&unicode).unwrap(), unicode);
    for name in ["a".repeat(240), "界".repeat(80)] {
        let original = format!("parent/{name}/child");
        match budget.path(&original) {
            Err(Error::ConversionNameBudget(NameBudgetError::ComponentTooLong {
                path, bytes, allowed,
            })) => {
                assert_eq!(path, PathBuf::from(original));
                assert_eq!(bytes, 240);
                assert_eq!(allowed, 239);
            }
            _ => panic!("over-budget component must preserve its original path and byte counts"),
        }
    }
}

#[test]
fn component_budget_handles_small_and_host_range_limits_without_a_default() {
    for maximum in [0, 1, 15, 16] {
        assert!(matches!(
            NameBudget::from_maximum(maximum),
            Err(Error::ConversionNameBudget(NameBudgetError::UnusableMaximum {
                maximum: observed,
            })) if observed == maximum
        ));
    }
    let smallest = NameBudget::from_maximum(17).unwrap();
    assert_eq!(smallest.path("a/b").unwrap(), "a/b");
    assert!(matches!(
        smallest.path("é"),
        Err(Error::ConversionNameBudget(NameBudgetError::ComponentTooLong {
            bytes: 2, allowed: 1, ..
        }))
    ));
    match usize::try_from(u64::MAX - 16) {
        Ok(allowed) => {
            assert_eq!(NameBudget::from_maximum(u64::MAX).unwrap().allowed, allowed);
        }
        Err(_) => assert!(matches!(
            NameBudget::from_maximum(u64::MAX),
            Err(Error::ConversionNameBudget(NameBudgetError::MaximumRange { .. }))
        )),
    }
}
