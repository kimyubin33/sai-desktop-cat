//! JSON 값을 다루는 작은 도우미. 화면과 주고받는 데이터는 예전 형식(JSON 객체)을 그대로 쓴다.
use serde_json::{Map, Value};

pub type Obj = Map<String, Value>;

/// 값 읽기 도우미
pub trait J {
    fn s(&self, k: &str) -> String;
    fn os(&self, k: &str) -> Option<String>;
    fn n(&self, k: &str) -> f64;
    fn on(&self, k: &str) -> Option<f64>;
    fn i(&self, k: &str) -> i64;
    fn b(&self, k: &str) -> bool;
    fn arr(&self, k: &str) -> Vec<Value>;
    fn strs(&self, k: &str) -> Vec<String>;
    fn obj(&self, k: &str) -> Obj;
}

impl J for Value {
    fn s(&self, k: &str) -> String {
        self.os(k).unwrap_or_default()
    }
    fn os(&self, k: &str) -> Option<String> {
        match self.get(k) {
            Some(Value::String(s)) => Some(s.clone()),
            Some(Value::Number(n)) => Some(n.to_string()),
            _ => None,
        }
    }
    fn n(&self, k: &str) -> f64 {
        self.on(k).unwrap_or(0.0)
    }
    fn on(&self, k: &str) -> Option<f64> {
        match self.get(k) {
            Some(Value::Number(n)) => n.as_f64(),
            Some(Value::String(s)) => s.trim().parse().ok(),
            Some(Value::Bool(b)) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        }
    }
    fn i(&self, k: &str) -> i64 {
        self.n(k) as i64
    }
    fn b(&self, k: &str) -> bool {
        match self.get(k) {
            Some(Value::Bool(b)) => *b,
            Some(Value::Null) | None => false,
            Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0) != 0.0,
            Some(Value::String(s)) => !s.is_empty(),
            Some(_) => true,
        }
    }
    fn arr(&self, k: &str) -> Vec<Value> {
        match self.get(k) {
            Some(Value::Array(a)) => a.clone(),
            _ => vec![],
        }
    }
    fn strs(&self, k: &str) -> Vec<String> {
        self.arr(k).iter().filter_map(|v| v.as_str().map(String::from)).collect()
    }
    fn obj(&self, k: &str) -> Obj {
        match self.get(k) {
            Some(Value::Object(o)) => o.clone(),
            _ => Obj::new(),
        }
    }
}

/// 값이 객체가 아니면 빈 객체로 만들고 그 객체를 돌려준다
pub fn ensure_obj(v: &mut Value) -> &mut Obj {
    if !v.is_object() {
        *v = Value::Object(Obj::new());
    }
    v.as_object_mut().unwrap()
}

/// v[k] 를 (없으면 default 로 만들어) 가변으로
pub fn entry<'a>(v: &'a mut Value, k: &str, default: Value) -> &'a mut Value {
    ensure_obj(v).entry(k.to_string()).or_insert(default)
}

/// v[k] 배열을 (없으면 만들어) 가변으로
pub fn arr_mut<'a>(v: &'a mut Value, k: &str) -> &'a mut Vec<Value> {
    let e = entry(v, k, Value::Array(vec![]));
    if !e.is_array() {
        *e = Value::Array(vec![]);
    }
    e.as_array_mut().unwrap()
}

pub fn set(v: &mut Value, k: &str, val: Value) {
    ensure_obj(v).insert(k.to_string(), val);
}

/// 공백을 하나로 줄이고 앞뒤를 자르고, 글자 수로 자른다 (JS 의 slice 와 같이 글자 기준)
pub fn clean(s: &str, max: usize) -> String {
    let joined = s.split_whitespace().collect::<Vec<_>>().join(" ");
    joined.chars().take(max).collect()
}

/// 글자 수 기준으로 자르기
pub fn take_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// 소수 첫째 자리까지
pub fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

pub fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// 숫자를 "3" 또는 "2.5" 처럼
pub fn fmt_h(h: f64) -> String {
    let r = round1(h);
    if (r - r.round()).abs() < 1e-9 {
        format!("{}", r.round() as i64)
    } else {
        format!("{:.1}", r)
    }
}

/// a 위에 b 를 깊게 덮어쓴다 (객체끼리만 재귀)
pub fn deep_merge(base: &mut Value, extra: &Value) {
    match (base, extra) {
        (Value::Object(b), Value::Object(e)) => {
            for (k, v) in e {
                match b.get_mut(k) {
                    Some(bv) if bv.is_object() && v.is_object() => deep_merge(bv, v),
                    _ => {
                        b.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (b, e) => *b = e.clone(),
    }
}

/// 짧은 무작위 id
pub fn rand_id(prefix: &str) -> String {
    use rand::Rng;
    let mut r = rand::thread_rng();
    format!("{}{}{}", prefix, crate::time::now_ms().max(0) as u64 % 0xffff_ffff, r.gen_range(0..1_000_000u32))
}

pub fn to_base36(mut n: u64) -> String {
    const D: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".into();
    }
    let mut out = vec![];
    while n > 0 {
        out.push(D[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap()
}
