//! Compiler from LiveMaker LSB `OpeData` expressions directly to NIR v2 stack bytecode (`nir_ir::Op`).
//!
//! Uses the verified LSB 116/117 operator table (opcodes 0..=20), SSA temporary local slots,
//! pure `BuiltinFn` list/string/math intrinsics, and persistent `scope == 3` profile sync.

use crate::lsb::{Expression, Literal};
use nir_ir::{BinOp, BuiltinFn, Op, StrId, UnaryOp, Value, VarId};
use std::collections::{BTreeMap, BTreeSet};

/// Host symbol/variable bridge provided by the LiveNovel compiler during expression lowering.
pub trait ExprBridge {
    fn stage_size(&self) -> [u32; 2];
    fn intern_str(&mut self, s: &str) -> StrId;
    /// Resolves or allocates a global `VarId` for a LiveMaker variable name.
    fn resolve_var(&mut self, name: &str) -> VarId;
    /// Returns true if the LiveMaker variable is already known/declared (for `fn50` VarExists).
    fn var_exists(&self, name: &str) -> bool;
    /// Returns the persistent profile key if `name` was declared with `scope == 3`.
    fn profile_key(&self, name: &str) -> Option<StrId>;
    /// Allocates or reuses a function-local slot for a `____*` SSA temporary.
    fn temp_local(&mut self, name: &str) -> u16;
    /// Records an `Adapted` diagnostic when a rarely used built-in is approximated.
    fn note_adapted(&mut self, msg: String);
}

#[derive(Clone, Debug)]
enum IndexedRef {
    Global(VarId, Literal),
    Local(u16, Literal),
}

/// Evaluates `expr` as a value expression, pushing its final result onto the VM stack.
/// If the expression is empty, pushes `Value::Null`.
pub fn compile_value_expr(expr: &Expression, bridge: &mut dyn ExprBridge, ops: &mut Vec<Op>) {
    if expr.operations.is_empty() {
        if let Some(lit) = &expr.literal {
            emit_literal(lit, bridge, ops, &BTreeSet::new());
        } else {
            ops.push(Op::PushNull);
        }
        return;
    }

    let mut temp_names: BTreeSet<String> = BTreeSet::new();
    let mut indexed_refs: BTreeMap<String, IndexedRef> = BTreeMap::new();
    let mut last_target: Option<String> = None;

    for (idx, (op_code, dst, operands)) in expr.operations.iter().enumerate() {
        let fn_id = expr.functions.get(&idx).copied();
        compile_single_op(
            *op_code,
            dst,
            operands,
            fn_id,
            &mut temp_names,
            &mut indexed_refs,
            bridge,
            ops,
        );
        last_target = Some(dst.clone());
    }

    // Push the final expression result (`____arg` or last assigned target).
    if let Some(target) = last_target {
        if target.is_empty() || target == "____arg" || is_temp_name(&target) {
            let slot = bridge.temp_local(&target);
            ops.push(Op::LoadLocal(slot));
        } else {
            let vid = bridge.resolve_var(&target);
            ops.push(Op::LoadVar(vid));
        }
    } else {
        ops.push(Op::PushNull);
    }
}

/// Compiles a `Calc` statement expression (all side effects executed, stack balanced).
pub fn compile_calc_stmt(expr: &Expression, bridge: &mut dyn ExprBridge, ops: &mut Vec<Op>) {
    if expr.operations.is_empty() {
        return;
    }
    let mut temp_names: BTreeSet<String> = BTreeSet::new();
    let mut indexed_refs: BTreeMap<String, IndexedRef> = BTreeMap::new();

    for (idx, (op_code, dst, operands)) in expr.operations.iter().enumerate() {
        let fn_id = expr.functions.get(&idx).copied();
        compile_single_op(
            *op_code,
            dst,
            operands,
            fn_id,
            &mut temp_names,
            &mut indexed_refs,
            bridge,
            ops,
        );
    }
}

/// Returns true if `expr` is statically known to evaluate to a truthy constant (e.g. `____arg = 1`).
pub fn is_const_true(expr: &Expression) -> bool {
    match &expr.literal {
        Some(Literal::Int(n)) => *n != 0,
        Some(Literal::Float(f)) => *f != 0.0,
        Some(Literal::String(s)) => !s.is_empty(),
        _ => expr.operations.is_empty(),
    }
}

/// Returns true if `expr` is statically known to evaluate to a falsy constant (e.g. `____arg = 0`).
pub fn is_const_false(expr: &Expression) -> bool {
    match &expr.literal {
        Some(Literal::Int(0)) => true,
        Some(Literal::String(s)) if s.is_empty() => true,
        _ => false,
    }
}

pub fn literal_string(expr: &Expression) -> Option<&str> {
    match &expr.literal {
        Some(Literal::String(s)) => Some(s.as_str()),
        _ => None,
    }
}

pub fn literal_i32(expr: &Expression) -> Option<i32> {
    match &expr.literal {
        Some(Literal::Int(n)) => Some(*n),
        Some(Literal::Float(f)) if f.is_finite() => Some(*f as i32),
        _ => None,
    }
}

/// Evaluates an expression to a static `Value` when it only uses constants, `@ScrWidth`, `@ScrHeight`.
pub fn eval_static_const(expr: &Expression, stage: [u32; 2]) -> Option<Value> {
    if let Some(lit) = &expr.literal {
        return match lit {
            Literal::Int(n) => Some(Value::Int(*n as i64)),
            Literal::Float(f) => Some(Value::Float(*f)),
            Literal::String(s) => Some(Value::Str(s.clone())),
            _ => None,
        };
    }
    if expr.operations.is_empty() {
        return Some(Value::Null);
    }
    let mut env: BTreeMap<String, Value> = BTreeMap::new();
    let resolve = |lit: &Literal, env: &BTreeMap<String, Value>| -> Option<Value> {
        match lit {
            Literal::Int(n) => Some(Value::Int(*n as i64)),
            Literal::Float(f) => Some(Value::Float(*f)),
            Literal::String(s) => Some(Value::Str(s.clone())),
            Literal::Variable(v) => match v.as_str() {
                "@ScrWidth" => Some(Value::Int(stage[0] as i64)),
                "@ScrHeight" => Some(Value::Int(stage[1] as i64)),
                _ => env.get(v).cloned(),
            },
            Literal::Unsupported => None,
        }
    };
    let mut last = None;
    for (idx, (op, dst, args)) in expr.operations.iter().enumerate() {
        let val = match *op {
            0 | 1 if args.len() == 1 => resolve(&args[0], &env)?,
            2 if args.len() == 2 => {
                let a = resolve(&args[0], &env)?.as_i64()?;
                let b = resolve(&args[1], &env)?.as_i64()?;
                Value::Int(a.saturating_add(b))
            }
            3 if args.len() == 2 => {
                let a = resolve(&args[0], &env)?.as_i64()?;
                let b = resolve(&args[1], &env)?.as_i64()?;
                Value::Int(a.saturating_sub(b))
            }
            4 if args.len() == 2 => {
                let a = resolve(&args[0], &env)?.as_i64()?;
                let b = resolve(&args[1], &env)?.as_i64()?;
                Value::Int(a.saturating_mul(b))
            }
            5 if args.len() == 2 => {
                let a = resolve(&args[0], &env)?.as_i64()?;
                let b = resolve(&args[1], &env)?.as_i64()?;
                if b == 0 {
                    return None;
                }
                Value::Int(a / b)
            }
            19 if args.len() == 2 => {
                let a = resolve(&args[0], &env)?.to_display_string();
                let b = resolve(&args[1], &env)?.to_display_string();
                Value::Str(format!("{a}{b}"))
            }
            11 => {
                let fn_id = expr.functions.get(&idx).copied()?;
                match fn_id {
                    35 => Value::Int(stage[0] as i64),
                    36 => Value::Int(stage[1] as i64),
                    37 if args.len() == 1 => {
                        Value::Int(resolve(&args[0], &env)?.as_i64().unwrap_or(0))
                    }
                    _ => return None,
                }
            }
            _ => return None,
        };
        env.insert(dst.clone(), val.clone());
        last = Some(val);
    }
    last
}

#[allow(clippy::too_many_arguments)]
fn compile_single_op(
    op_code: u8,
    dst: &str,
    operands: &[Literal],
    fn_id: Option<u8>,
    temp_names: &mut BTreeSet<String>,
    indexed_refs: &mut BTreeMap<String, IndexedRef>,
    bridge: &mut dyn ExprBridge,
    ops: &mut Vec<Op>,
) {
    // Handle array-element store pattern: `____d_0 = ([] arr, idx); ____d_0 = rhs`
    if op_code == 1 && operands.len() == 1 {
        if let Some(iref) = indexed_refs.get(dst).cloned() {
            match iref {
                IndexedRef::Global(vid, idx_lit) => {
                    ops.push(Op::LoadVar(vid));
                    emit_literal(&idx_lit, bridge, ops, temp_names);
                    emit_literal(&operands[0], bridge, ops, temp_names);
                    ops.push(Op::CallBuiltin {
                        func: BuiltinFn::ListSet,
                        argc: 3,
                    });
                    ops.push(Op::StoreVar(vid));
                }
                IndexedRef::Local(slot, idx_lit) => {
                    ops.push(Op::LoadLocal(slot));
                    emit_literal(&idx_lit, bridge, ops, temp_names);
                    emit_literal(&operands[0], bridge, ops, temp_names);
                    ops.push(Op::CallBuiltin {
                        func: BuiltinFn::ListSet,
                        argc: 3,
                    });
                    ops.push(Op::StoreLocal(slot));
                }
            }
            return;
        }
    }

    match op_code {
        0 | 1 => {
            if let Some(arg) = operands.first() {
                emit_literal(arg, bridge, ops, temp_names);
            } else {
                ops.push(Op::PushNull);
            }
            store_dst(dst, temp_names, bridge, ops);
        }
        2..=9 | 12..=20 => {
            let bin = match op_code {
                2 => BinOp::Add,
                3 => BinOp::Sub,
                4 => BinOp::Mul,
                5 => BinOp::Div,
                6 => BinOp::Rem,
                7 => BinOp::BitOr,
                8 => BinOp::BitAnd,
                9 => BinOp::BitXor,
                12 => BinOp::Eq,
                13 => BinOp::Gt,
                14 => BinOp::Lt,
                15 => BinOp::Ge,
                16 => BinOp::Le,
                17 => BinOp::Shl,
                18 => BinOp::Shr,
                19 => BinOp::Concat,
                20 => BinOp::Ne,
                _ => unreachable!(),
            };
            if operands.len() >= 2 {
                emit_literal(&operands[0], bridge, ops, temp_names);
                emit_literal(&operands[1], bridge, ops, temp_names);
                ops.push(Op::BinOp(bin));
            } else if operands.len() == 1 && op_code == 3 {
                // Unary negation fallback
                emit_literal(&operands[0], bridge, ops, temp_names);
                ops.push(Op::UnaryOp(UnaryOp::Neg));
            } else {
                ops.push(Op::PushInt(0));
            }
            store_dst(dst, temp_names, bridge, ops);
        }
        10 => {
            // Array indexing `dst = operands[0][operands[1]]`
            if operands.len() >= 2 {
                if is_temp_name(dst) {
                    if let Literal::Variable(arr_name) = &operands[0] {
                        if arr_name == "@ParamStr" {
                            indexed_refs
                                .insert(dst.to_owned(), IndexedRef::Local(0, operands[1].clone()));
                        } else if is_temp_name(arr_name) {
                            let slot = bridge.temp_local(arr_name);
                            indexed_refs.insert(
                                dst.to_owned(),
                                IndexedRef::Local(slot, operands[1].clone()),
                            );
                        } else {
                            let vid = bridge.resolve_var(arr_name);
                            indexed_refs.insert(
                                dst.to_owned(),
                                IndexedRef::Global(vid, operands[1].clone()),
                            );
                        }
                    }
                }
                emit_literal(&operands[0], bridge, ops, temp_names);
                emit_literal(&operands[1], bridge, ops, temp_names);
                ops.push(Op::CallBuiltin {
                    func: BuiltinFn::ListGet,
                    argc: 2,
                });
            } else {
                ops.push(Op::PushNull);
            }
            store_dst(dst, temp_names, bridge, ops);
        }
        11 => {
            let fid = fn_id.unwrap_or(0);
            compile_builtin_func(fid, operands, temp_names, bridge, ops);
            store_dst(dst, temp_names, bridge, ops);
        }
        other => {
            bridge.note_adapted(format!(
                "unknown LSB expression operator {other} evaluated to 0"
            ));
            ops.push(Op::PushInt(0));
            store_dst(dst, temp_names, bridge, ops);
        }
    }
}

fn compile_builtin_func(
    fn_id: u8,
    operands: &[Literal],
    temp_names: &BTreeSet<String>,
    bridge: &mut dyn ExprBridge,
    ops: &mut Vec<Op>,
) {
    match fn_id {
        // fn2: GetProp(obj, prop_id) -> in story scripts, mostly Wait guard on メッセージボックス (busy = 0)
        2 => {
            ops.push(Op::PushInt(0));
        }
        // fn4: GetArraySize(arr)
        4 => {
            if let Some(a) = operands.first() {
                emit_literal(a, bridge, ops, temp_names);
            } else {
                ops.push(Op::PushNull);
            }
            ops.push(Op::CallBuiltin {
                func: BuiltinFn::ListLen,
                argc: 1,
            });
        }
        // fn19: Exists(obj_name) -> false in story VM context
        19 => {
            ops.push(Op::PushInt(0));
        }
        // fn20: Not(x)
        20 => {
            if let Some(a) = operands.first() {
                emit_literal(a, bridge, ops, temp_names);
            } else {
                ops.push(Op::PushInt(0));
            }
            ops.push(Op::UnaryOp(UnaryOp::Not));
        }
        // fn26: Random(n) -> 0..n-1
        26 => {
            if let Some(a) = operands.first() {
                emit_literal(a, bridge, ops, temp_names);
            } else {
                ops.push(Op::PushInt(1));
            }
            ops.push(Op::CallBuiltin {
                func: BuiltinFn::RandInt,
                argc: 1,
            });
        }
        // fn28: ArrayToString(arr [, delim])
        28 => {
            if let Some(a) = operands.first() {
                emit_literal(a, bridge, ops, temp_names);
            } else {
                ops.push(Op::PushNull);
            }
            if let Some(d) = operands.get(1) {
                emit_literal(d, bridge, ops, temp_names);
                ops.push(Op::CallBuiltin {
                    func: BuiltinFn::ListJoin,
                    argc: 2,
                });
            } else {
                ops.push(Op::CallBuiltin {
                    func: BuiltinFn::ListJoin,
                    argc: 1,
                });
            }
        }
        // fn29: StringToArray(src_str, dst_array_var [, delim])
        29 => {
            if operands.len() >= 2 {
                emit_literal(&operands[0], bridge, ops, temp_names);
                if let Some(d) = operands.get(2) {
                    emit_literal(d, bridge, ops, temp_names);
                    ops.push(Op::CallBuiltin {
                        func: BuiltinFn::ListSplit,
                        argc: 2,
                    });
                } else {
                    ops.push(Op::CallBuiltin {
                        func: BuiltinFn::ListSplit,
                        argc: 1,
                    });
                }
                if let Literal::Variable(dst_var) = &operands[1] {
                    store_var_direct(dst_var, temp_names, bridge, ops);
                } else {
                    ops.push(Op::Pop);
                }
            }
            ops.push(Op::PushInt(1));
        }
        // fn35 / fn36: GetScreenX / GetScreenY
        35 => {
            ops.push(Op::PushInt(bridge.stage_size()[0] as i64));
        }
        36 => {
            ops.push(Op::PushInt(bridge.stage_size()[1] as i64));
        }
        // fn37: StrToInt / Int(x)
        37 => {
            if let Some(a) = operands.first() {
                emit_literal(a, bridge, ops, temp_names);
            } else {
                ops.push(Op::PushInt(0));
            }
            ops.push(Op::CallBuiltin {
                func: BuiltinFn::ToInt,
                argc: 1,
            });
        }
        // fn50: VarExists(var_name)
        50 => {
            let exists = match operands.first() {
                Some(Literal::String(name)) => bridge.var_exists(name),
                Some(Literal::Variable(_)) => true,
                _ => false,
            };
            ops.push(Op::PushInt(i64::from(exists)));
        }
        // fn66: AddArray(arr_var, item) -> mutates arr_var in place
        66 => {
            if operands.len() >= 2 {
                emit_literal(&operands[0], bridge, ops, temp_names);
                emit_literal(&operands[1], bridge, ops, temp_names);
                ops.push(Op::CallBuiltin {
                    func: BuiltinFn::ListPush,
                    argc: 2,
                });
                if let Literal::Variable(dst_var) = &operands[0] {
                    store_var_direct(dst_var, temp_names, bridge, ops);
                } else {
                    ops.push(Op::Pop);
                }
            }
            ops.push(Op::PushInt(1));
        }
        // fn126: Min(a, b), fn127: Max(a, b)
        126 | 127 => {
            let func = if fn_id == 126 {
                BuiltinFn::Min
            } else {
                BuiltinFn::Max
            };
            if operands.len() >= 2 {
                emit_literal(&operands[0], bridge, ops, temp_names);
                emit_literal(&operands[1], bridge, ops, temp_names);
                ops.push(Op::CallBuiltin { func, argc: 2 });
            } else {
                ops.push(Op::PushInt(0));
            }
        }
        // fn133: AddDelimiter(delim, str)
        133 => {
            if operands.len() >= 2 {
                emit_literal(&operands[0], bridge, ops, temp_names);
                emit_literal(&operands[1], bridge, ops, temp_names);
                ops.push(Op::CallBuiltin {
                    func: BuiltinFn::AddDelimiter,
                    argc: 2,
                });
            } else {
                ops.push(Op::PushStr(bridge.intern_str("")));
            }
        }
        // fn144: SetHistoryMaxCount (no-op)
        144 => {
            ops.push(Op::PushInt(0));
        }
        // fn165: ArrayCompact(arr_var) -> removes empty elements in place
        165 => {
            if let Some(arg) = operands.first() {
                emit_literal(arg, bridge, ops, temp_names);
                ops.push(Op::CallBuiltin {
                    func: BuiltinFn::ListCompact,
                    argc: 1,
                });
                if let Literal::Variable(dst_var) = arg {
                    store_var_direct(dst_var, temp_names, bridge, ops);
                } else {
                    ops.push(Op::Pop);
                }
            }
            ops.push(Op::PushInt(1));
        }
        other => {
            bridge.note_adapted(format!("LiveMaker built-in fn{other} evaluated to 0"));
            ops.push(Op::PushInt(0));
        }
    }
}

fn emit_literal(
    lit: &Literal,
    bridge: &mut dyn ExprBridge,
    ops: &mut Vec<Op>,
    temp_names: &BTreeSet<String>,
) {
    match lit {
        Literal::Int(n) => ops.push(Op::PushInt(*n as i64)),
        Literal::Float(f) => ops.push(Op::PushFloat(*f)),
        Literal::String(s) => {
            let sid = bridge.intern_str(s);
            ops.push(Op::PushStr(sid));
        }
        Literal::Variable(name) => match name.as_str() {
            "@ScrWidth" => ops.push(Op::PushInt(bridge.stage_size()[0] as i64)),
            "@ScrHeight" => ops.push(Op::PushInt(bridge.stage_size()[1] as i64)),
            "@ParamStr" => ops.push(Op::LoadLocal(0)),
            _ if is_temp_name(name) || temp_names.contains(name) => {
                let slot = bridge.temp_local(name);
                ops.push(Op::LoadLocal(slot));
            }
            _ => {
                let vid = bridge.resolve_var(name);
                ops.push(Op::LoadVar(vid));
            }
        },
        Literal::Unsupported => ops.push(Op::PushNull),
    }
}

fn store_dst(
    dst: &str,
    temp_names: &mut BTreeSet<String>,
    bridge: &mut dyn ExprBridge,
    ops: &mut Vec<Op>,
) {
    if dst.is_empty() || is_temp_name(dst) {
        temp_names.insert(dst.to_owned());
        let slot = bridge.temp_local(dst);
        ops.push(Op::StoreLocal(slot));
    } else {
        let vid = bridge.resolve_var(dst);
        ops.push(Op::StoreVar(vid));
        if let Some(key) = bridge.profile_key(dst) {
            ops.push(Op::LoadVar(vid));
            ops.push(Op::ProfileSetVal { key });
        }
    }
}

fn store_var_direct(
    dst: &str,
    temp_names: &BTreeSet<String>,
    bridge: &mut dyn ExprBridge,
    ops: &mut Vec<Op>,
) {
    if dst.is_empty() || is_temp_name(dst) || temp_names.contains(dst) {
        let slot = bridge.temp_local(dst);
        ops.push(Op::StoreLocal(slot));
    } else {
        let vid = bridge.resolve_var(dst);
        ops.push(Op::StoreVar(vid));
        if let Some(key) = bridge.profile_key(dst) {
            ops.push(Op::LoadVar(vid));
            ops.push(Op::ProfileSetVal { key });
        }
    }
}

pub fn is_temp_name(name: &str) -> bool {
    name.starts_with("____")
}
