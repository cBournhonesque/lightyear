/// Returns whether or not a wrapping number is greater than another
/// sequence_greater_than(2,1) will return true
/// sequence_greater_than(1,2) will return false
/// sequence_greater_than(1,1) will return false
pub fn sequence_greater_than(s1: u16, s2: u16) -> bool {
    ((s1 > s2) && (s1 - s2 <= 32768)) || ((s1 < s2) && (s2 - s1 > 32768))
}

/// Returns whether or not a wrapping number is greater than another
/// sequence_less_than(1,2) will return true
/// sequence_less_than(2,1) will return false
/// sequence_less_than(1,1) will return false
pub fn sequence_less_than(s1: u16, s2: u16) -> bool {
    sequence_greater_than(s2, s1)
}

/// Retrieves the wrapping difference between 2 u16 values
/// wrapping_diff(1,2) will return 1
/// wrapping_diff(2,1) will return -1
/// wrapping_diff(65535,0) will return 1
/// wrapping_diff(0,65535) will return -1
pub fn wrapping_diff(a: u16, b: u16) -> i16 {
    const MAX: i32 = std::i16::MAX as i32;
    const MIN: i32 = std::i16::MIN as i32;
    const ADJUST: i32 = (std::u16::MAX as i32) + 1;

    let a: i32 = i32::from(a);
    let b: i32 = i32::from(b);

    let mut result = b - a;
    if (MIN..=MAX).contains(&result) {
        result as i16
    } else if b > a {
        result = b - (a + ADJUST);
        if (MIN..=MAX).contains(&result) {
            result as i16
        } else {
            panic!("integer overflow, this shouldn't happen")
        }
    } else {
        result = (b + ADJUST) - a;
        if (MIN..=MAX).contains(&result) {
            result as i16
        } else {
            panic!("integer overflow, this shouldn't happen")
        }
    }
}

#[cfg(test)]
mod sequence_compare_tests {
    use super::{sequence_greater_than, sequence_less_than};

    #[test]
    fn greater_is_greater() {
        assert!(sequence_greater_than(2, 1));
    }

    #[test]
    fn greater_is_not_equal() {
        assert!(!sequence_greater_than(2, 2));
    }

    #[test]
    fn greater_is_not_less() {
        assert!(!sequence_greater_than(1, 2));
    }

    #[test]
    fn less_is_less() {
        assert!(sequence_less_than(1, 2));
    }

    #[test]
    fn less_is_not_equal() {
        assert!(!sequence_less_than(2, 2));
    }

    #[test]
    fn less_is_not_greater() {
        assert!(!sequence_less_than(2, 1));
    }
}

#[cfg(test)]
mod wrapping_diff_tests {
    use super::wrapping_diff;

    #[test]
    fn simple() {
        let a: u16 = 10;
        let b: u16 = 12;

        let result = wrapping_diff(a, b);

        assert_eq!(result, 2);
    }

    #[test]
    fn simple_backwards() {
        let a: u16 = 10;
        let b: u16 = 12;

        let result = wrapping_diff(b, a);

        assert_eq!(result, -2);
    }

    #[test]
    fn max_wrap() {
        let a: u16 = std::u16::MAX;
        let b: u16 = a.wrapping_add(2);

        let result = wrapping_diff(a, b);

        assert_eq!(result, 2);
    }

    #[test]
    fn min_wrap() {
        let a: u16 = 0;
        let b: u16 = a.wrapping_sub(2);

        let result = wrapping_diff(a, b);

        assert_eq!(result, -2);
    }

    #[test]
    fn max_wrap_backwards() {
        let a: u16 = std::u16::MAX;
        let b: u16 = a.wrapping_add(2);

        let result = wrapping_diff(b, a);

        assert_eq!(result, -2);
    }

    #[test]
    fn min_wrap_backwards() {
        let a: u16 = 0;
        let b: u16 = a.wrapping_sub(2);

        let result = wrapping_diff(b, a);

        assert_eq!(result, 2);
    }

    #[test]
    fn medium_min_wrap() {
        let diff: u16 = std::u16::MAX / 2;
        let a: u16 = 0;
        let b: u16 = a.wrapping_sub(diff);

        let result = i32::from(wrapping_diff(a, b));

        assert_eq!(result, -i32::from(diff));
    }

    #[test]
    fn medium_min_wrap_backwards() {
        let diff: u16 = std::u16::MAX / 2;
        let a: u16 = 0;
        let b: u16 = a.wrapping_sub(diff);

        let result = i32::from(wrapping_diff(b, a));

        assert_eq!(result, i32::from(diff));
    }

    #[test]
    fn medium_max_wrap() {
        let diff: u16 = std::u16::MAX / 2;
        let a: u16 = std::u16::MAX;
        let b: u16 = a.wrapping_add(diff);

        let result = i32::from(wrapping_diff(a, b));

        assert_eq!(result, i32::from(diff));
    }

    #[test]
    fn medium_max_wrap_backwards() {
        let diff: u16 = std::u16::MAX / 2;
        let a: u16 = std::u16::MAX;
        let b: u16 = a.wrapping_add(diff);

        let result = i32::from(wrapping_diff(b, a));

        assert_eq!(result, -i32::from(diff));
    }
}
