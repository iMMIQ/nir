use super::*;

impl Core {
    pub(super) fn read(&self, name: &str) -> Result<Value> {
        self.frame()
            .locals
            .get(name)
            .or_else(|| self.state.variables.get(name))
            .cloned()
            .ok_or_else(|| self.error("E_UNINITIALIZED", name))
    }
    pub(super) fn write(&mut self, name: &str, v: Value) -> Result<()> {
        if self.state.variables.contains_key(name) {
            self.state.variables.insert(name.into(), v);
        } else {
            self.frame_mut().locals.insert(name.into(), v);
        }
        Ok(())
    }
    pub fn eval(&self, e: &Expr) -> Result<Value> {
        use BinaryOp::*;
        use Value::*;
        match e {
            Expr::Const { value } => Ok(value.clone()),
            Expr::Var { name } => self.read(name),
            Expr::ToF80 { value } => match self.eval(value)? {
                I32(v) => Ok(F80(Float80::from_i32(v))),
                F80(v) => Ok(F80(v)),
                _ => Err(self.error("E_TYPE", "to_f80 requires numeric input")),
            },
            Expr::ToI32 { value } => match self.eval(value)? {
                I32(v) => Ok(I32(v)),
                F80(v) => v
                    .to_i32()
                    .map(I32)
                    .ok_or_else(|| self.error("E_ARITHMETIC", "integer conversion overflow")),
                _ => Err(self.error("E_TYPE", "to_i32 requires numeric input")),
            },
            Expr::Not { value } => match self.eval(value)? {
                Bool(b) => Ok(Bool(!b)),
                _ => Err(self.error("E_TYPE", "not")),
            },
            Expr::Binary { op, left, right } => {
                let l = self.eval(left)?;
                if matches!((op, &l), (And, Bool(false))) {
                    return Ok(Bool(false));
                }
                if matches!((op, &l), (Or, Bool(true))) {
                    return Ok(Bool(true));
                }
                let r = self.eval(right)?;
                let overflow = || self.error("E_ARITHMETIC", "overflow or division by zero");
                match (op, l, r) {
                    (Add, I32(a), I32(b)) => a.checked_add(b).map(I32).ok_or_else(overflow),
                    (Sub, I32(a), I32(b)) => a.checked_sub(b).map(I32).ok_or_else(overflow),
                    (Mul, I32(a), I32(b)) => a.checked_mul(b).map(I32).ok_or_else(overflow),
                    (Div, I32(a), I32(b)) => a.checked_div(b).map(I32).ok_or_else(overflow),
                    (Rem, I32(a), I32(b)) => a.checked_rem(b).map(I32).ok_or_else(overflow),
                    (op @ (Add | Sub | Mul | Div | Rem), F80(a), F80(b)) => {
                        a.checked_binary(*op, b).map(F80).ok_or_else(overflow)
                    }
                    (Lt, F80(a), F80(b)) => Ok(Bool(a < b)),
                    (Le, F80(a), F80(b)) => Ok(Bool(a <= b)),
                    (Gt, F80(a), F80(b)) => Ok(Bool(a > b)),
                    (Ge, F80(a), F80(b)) => Ok(Bool(a >= b)),
                    (Eq, a, b) => Ok(Bool(a == b)),
                    (Ne, a, b) => Ok(Bool(a != b)),
                    (Lt, I32(a), I32(b)) => Ok(Bool(a < b)),
                    (Le, I32(a), I32(b)) => Ok(Bool(a <= b)),
                    (Gt, I32(a), I32(b)) => Ok(Bool(a > b)),
                    (Ge, I32(a), I32(b)) => Ok(Bool(a >= b)),
                    (And, Bool(a), Bool(b)) => Ok(Bool(a && b)),
                    (Or, Bool(a), Bool(b)) => Ok(Bool(a || b)),
                    (Concat, String(a), String(b)) => {
                        if a.len() + b.len() > 128 * 1024 {
                            return Err(self.error("E_LIMIT", "string length"));
                        }
                        Ok(String(a + &b))
                    }
                    _ => Err(self.error("E_TYPE", "binary operands")),
                }
            }
        }
    }
    pub(super) fn predicate(&self, e: &Option<Expr>) -> Result<bool> {
        match e {
            None => Ok(true),
            Some(e) => match self.eval(e)? {
                Value::Bool(b) => Ok(b),
                _ => Err(self.error("E_TYPE", "predicate")),
            },
        }
    }
    pub(super) fn freeze_text(&self, id: &str) -> Result<Vec<FrozenSpan>> {
        let doc = self.program().locales[&self.state.locale]
            .get(id)
            .ok_or_else(|| self.error("E_TEXT", id))?;
        doc.spans
            .iter()
            .map(|s| {
                Ok(match s {
                    Span::Image { id, image } => FrozenSpan {
                        id: id.clone(),
                        text: "\u{fffc}".into(),
                        emphasis: false,
                        gate: false,
                        pause: false,
                        pause_timeout_us: None,
                        ruby: None,
                        image: Some(image.clone()),
                    },
                    Span::Ruby { id, text, reading } => FrozenSpan {
                        id: id.clone(),
                        text: text.clone(),
                        emphasis: false,
                        gate: false,
                        pause: false,
                        pause_timeout_us: None,
                        ruby: Some(reading.clone()),
                        image: None,
                    },
                    Span::Text { id, text, emphasis } => FrozenSpan {
                        id: id.clone(),
                        text: text.clone(),
                        emphasis: *emphasis,
                        gate: false,
                        pause: false,
                        pause_timeout_us: None,
                        ruby: None,
                        image: None,
                    },
                    Span::Break { id } => FrozenSpan {
                        id: id.clone(),
                        text: "\n".into(),
                        emphasis: false,
                        gate: false,
                        pause: false,
                        pause_timeout_us: None,
                        ruby: None,
                        image: None,
                    },
                    Span::Gate { id } => FrozenSpan {
                        id: id.clone(),
                        text: String::new(),
                        emphasis: false,
                        gate: true,
                        pause: false,
                        pause_timeout_us: None,
                        ruby: None,
                        image: None,
                    },
                    Span::Pause { id, timeout_us } => FrozenSpan {
                        id: id.clone(),
                        text: String::new(),
                        emphasis: false,
                        gate: false,
                        pause: true,
                        pause_timeout_us: *timeout_us,
                        ruby: None,
                        image: None,
                    },
                    Span::Param { id, name } => {
                        let v = match self.read(name)? {
                            Value::String(v) => v,
                            Value::I32(v) => v.to_string(),
                            Value::Bool(v) => v.to_string(),
                            Value::F80(v) => v.to_string(),
                        };
                        FrozenSpan {
                            id: id.clone(),
                            text: format!("\u{2068}{v}\u{2069}"),
                            emphasis: false,
                            gate: false,
                            pause: false,
                            pause_timeout_us: None,
                            ruby: None,
                            image: None,
                        }
                    }
                })
            })
            .collect()
    }
    pub(super) fn text(&self, id: &str) -> Result<String> {
        Ok(self
            .freeze_text(id)?
            .iter()
            .map(|s| s.text.as_str())
            .collect())
    }
}
