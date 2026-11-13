//! Port of `src/ucioption.{h,cpp}`: the UCI options, their parsing and
//! their printing, with the names, types, defaults and bounds of C++.
//! Names are compared without case, as in C++. The engine acts on a change
//! after `set()` returns (C++ calls an `on_change` function).

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OptionType {
    Check,
    Spin,
    Combo,
    Button,
    String,
}

pub struct UciOption {
    pub name: &'static str,
    pub kind: OptionType,
    pub default_value: String,
    pub current_value: String,
    pub min: i64,
    pub max: i64,
}

impl UciOption {
    pub fn check(name: &'static str, v: bool) -> UciOption {
        let s = if v { "true" } else { "false" };
        UciOption { name, kind: OptionType::Check, default_value: s.into(), current_value: s.into(), min: 0, max: 0 }
    }

    pub fn spin(name: &'static str, v: i64, min: i64, max: i64) -> UciOption {
        UciOption { name, kind: OptionType::Spin, default_value: v.to_string(), current_value: v.to_string(), min, max }
    }

    pub fn string(name: &'static str, v: &str) -> UciOption {
        UciOption { name, kind: OptionType::String, default_value: v.into(), current_value: v.into(), min: 0, max: 0 }
    }

    pub fn button(name: &'static str) -> UciOption {
        UciOption {
            name,
            kind: OptionType::Button,
            default_value: String::new(),
            current_value: String::new(),
            min: 0,
            max: 0,
        }
    }

    pub fn combo(name: &'static str, v: &str, cur: &str) -> UciOption {
        UciOption { name, kind: OptionType::Combo, default_value: v.into(), current_value: cur.into(), min: 0, max: 0 }
    }

    /// `operator int()`: a spin or check value.
    pub fn int(&self) -> i64 {
        match self.kind {
            OptionType::Spin => self.current_value.parse().unwrap_or(0),
            _ => (self.current_value == "true") as i64,
        }
    }

    /// `Option::operator=`: returns whether the value was accepted (and the
    /// `on_change` action is due).
    fn assign(&mut self, v: &str) -> bool {
        if (self.kind != OptionType::Button && self.kind != OptionType::String && v.is_empty())
            || (self.kind == OptionType::Check && v != "true" && v != "false")
            || (self.kind == OptionType::Spin && !value_in_range(v, self.min, self.max))
        {
            return false;
        }

        if self.kind == OptionType::Combo {
            let ok = self.default_value.split_whitespace().any(|t| t.eq_ignore_ascii_case(v)) && v != "var";
            if !ok {
                return false;
            }
        }

        if self.kind == OptionType::String {
            self.current_value = if v == "<empty>" { String::new() } else { v.to_string() };
        } else if self.kind != OptionType::Button {
            self.current_value = v.to_string();
        }
        true
    }
}

/// `value_in_range()`: `strtoll` of the whole string, within the bounds.
fn value_in_range(v: &str, min: i64, max: i64) -> bool {
    if v.is_empty() {
        return false;
    }
    // strtoll skips leading white space and takes an optional sign
    let t = v.trim_start_matches([' ', '\t', '\n', '\x0B', '\x0C', '\r']);
    let digits = t.strip_prefix(['+', '-']).unwrap_or(t);
    if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return false;
    }
    match t.parse::<i64>() {
        Ok(r) => r >= min && r <= max,
        Err(_) => false, // ERANGE
    }
}

#[derive(Default)]
pub struct OptionsMap {
    options: Vec<UciOption>,
}

impl OptionsMap {
    pub fn add(&mut self, o: UciOption) {
        assert!(self.find(o.name).is_none(), "Option \"{}\" was already added!", o.name);
        self.options.push(o);
    }

    fn find(&self, name: &str) -> Option<usize> {
        self.options.iter().position(|o| o.name.eq_ignore_ascii_case(name))
    }

    pub fn get(&self, name: &str) -> &UciOption {
        &self.options[self.find(name).expect("an option")]
    }

    pub fn int(&self, name: &str) -> i64 {
        self.get(name).int()
    }

    pub fn string(&self, name: &str) -> &str {
        &self.get(name).current_value
    }

    /// `setoption name <name> value <value>`: sets the option, and returns
    /// its name if the value was accepted, for the engine to act on it;
    /// prints "No such option" for an unknown name.
    pub fn setoption(&mut self, name: &str, value: &str) -> Option<&'static str> {
        match self.find(name) {
            Some(i) => {
                let o = &mut self.options[i];
                if o.assign(value) {
                    Some(o.name)
                } else {
                    None
                }
            }
            None => {
                println!("No such option: {name}");
                None
            }
        }
    }

    /// `operator<<`: the options as `uci` lists them.
    pub fn uci_list(&self) -> String {
        let mut s = String::new();
        for o in &self.options {
            s += &format!("\noption name {} type ", o.name);
            match o.kind {
                OptionType::Check => s += &format!("check default {}", o.default_value),
                OptionType::Combo => s += &format!("combo default {}", o.default_value),
                OptionType::String => {
                    let d = if o.default_value.is_empty() { "<empty>" } else { &o.default_value };
                    s += &format!("string default {d}");
                }
                OptionType::Spin => {
                    s += &format!("spin default {} min {} max {}", o.default_value, o.min, o.max);
                }
                OptionType::Button => s += "button",
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spin_values_like_strtoll() {
        assert!(value_in_range("16", 1, 100));
        assert!(value_in_range("+16", 1, 100));
        assert!(value_in_range(" 16", 1, 100));
        assert!(!value_in_range("16 ", 1, 100));
        assert!(!value_in_range("1e3", 1, 10000));
        assert!(!value_in_range("0", 1, 100));
        assert!(!value_in_range("99999999999999999999", 1, 100));
    }
}
