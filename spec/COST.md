# The cost register

Every priced operation in the reducer and its builtins, the work it does, and the shape of the charge
against it. Authored in `spec/cost-register.tsv`, rendered here, checked by
`tools/check-cost-register.py` — an arm with no row, a row for nothing, and a declared charge that is
not where the row says are all build failures.

**What this is for.** The "flat charge for superlinear work" defect has been found twice — September
2026's union dedup, October 2026's collection conversions, union asymmetry and `censure` — and both
times by measurement, because nothing in the tree could see a charge that does not track its work. This
is that instrument's data. It makes each charge a *declared* shape a reviewer can read, and
`rholang/tests/cost_shapes.rs` measures the charge through the reducer so a shape that stops growing
fails. Passing the gate does not prove the work matches the charge: the charge is linear for `O(n log
n)` work, which is the recorded log-factor residual, and a `capped` operation is bounded by a constant
rather than priced by phlo.

| id | operation(s) | shape | basis | charge | evidence |
|---|---|---|---|---|---|
| COST001 | `nth` | constant | - | `nth_method_call_cost` | rholang/tests/cost_shapes.rs::constant_operations_do_not_grow_with_the_operand |
| COST002 | `toInt` | linear | operand size | `to_int_cost_bigint,to_int_cost_string` | rholang/tests/cost_shapes.rs::linear_operations_grow_with_their_operand |
| COST003 | `toBigInt` | linear | operand size | `int_to_bigint_cost,to_bigint_cost,to_int_cost_bigint` | rholang/tests/cost_shapes.rs::linear_operations_grow_with_their_operand |
| COST004 | `hexToBytes,toUtf8Bytes` | linear | string length | `hex_to_bytes_cost` | rholang/tests/cost_shapes.rs::linear_operations_grow_with_their_operand |
| COST005 | `bytesToHex` | linear | byte length | `bytes_to_hex_cost` | rholang/tests/cost_shapes.rs::linear_operations_grow_with_their_operand |
| COST006 | `toByteArray` | linear | serialized size | `to_byte_array_cost` | rholang/tests/cost_shapes.rs::linear_operations_grow_with_their_operand |
| COST007 | `union` | linear | both operand counts | `union_cost` | rholang/src/reduce.rs::a_union_charges_both_operands_alike |
| COST008 | `diff` | product | both operand counts | `diff_cost` | spec/RUST-VS-SCALA.md |
| COST009 | `add` | linear | the rebuilt set size | `helper:set_add` | rholang/src/reduce.rs::a_set_add_charges_the_set_it_rebuilds |
| COST010 | `delete` | linear | the operand count | `helper:collection_delete` | rholang/tests/cost_shapes.rs::linear_operations_grow_with_their_operand |
| COST011 | `contains` | product | haystack x needle for strings, operand count for collections | `lookup_cost,string_search_worst_cost` | rholang/tests/cost_shapes.rs::product_operations_grow_with_both_operands |
| COST012 | `get,getOrElse` | linear | map size | `lookup_cost` | rholang/tests/cost_shapes.rs::linear_operations_grow_with_their_operand |
| COST013 | `set` | linear | the rebuilt map size | `canonicalization_cost` | rholang/src/reduce.rs::set_charges_the_map_it_rebuilds |
| COST014 | `keys` | linear | key count | `canonicalization_cost` | rholang/src/reduce.rs::keys_charges_its_output |
| COST015 | `size` | linear | the collection size | `size_method_cost` | rholang/tests/cost_shapes.rs::linear_operations_grow_with_their_operand |
| COST016 | `length` | linear | string length | `length_method_cost` | rholang/tests/cost_shapes.rs::constant_operations_do_not_grow_with_the_operand |
| COST017 | `slice,substring` | linear | the slice | `slice_cost` | rholang/tests/cost_shapes.rs::linear_operations_grow_with_their_operand |
| COST018 | `take` | linear | the prefix | `take_cost` | rholang/tests/cost_shapes.rs::linear_operations_grow_with_their_operand |
| COST019 | `toList` | linear | the output count | `to_list_cost` | rholang/tests/cost_shapes.rs::linear_operations_grow_with_their_operand |
| COST020 | `toSet,toMap` | linear | the output count | `canonicalization_cost` | rholang/src/reduce.rs::to_set_charges_its_elements,rholang/src/reduce.rs::to_map_charges_its_elements |
| COST021 | `indexOf` | product | haystack x needle | `string_search_worst_cost` | rholang/tests/cost_shapes.rs::product_operations_grow_with_both_operands |
| COST022 | `toLowerCase,toUpperCase,capitalize,reverse,trim` | linear | string length | `string_transform_cost` | rholang/tests/cost_shapes.rs::linear_operations_grow_with_their_operand |
| COST023 | `isEmpty,nonEmpty` | identity | - | `-` | rholang/tests/cost_shapes.rs::constant_operations_do_not_grow_with_the_operand |
| COST024 | `startsWith,endsWith` | linear | haystack + needle | `string_search_cost` | rholang/tests/cost_shapes.rs::linear_operations_grow_with_their_operand |
| COST025 | `replace` | linear | the output bound | `string_replace_cost` | rholang/tests/cost_shapes.rs::linear_operations_grow_with_their_operand |
| COST026 | `split` | linear | input + separator | `string_split_cost` | rholang/tests/cost_shapes.rs::linear_operations_grow_with_their_operand |
| COST027 | `format` | product | format x arguments | `string_format_cost` | rholang/tests/cost_shapes.rs::product_operations_grow_with_both_operands |
| COST028 | `toString` | linear | the output length | `to_string_cost` | rholang/tests/cost_shapes.rs::linear_operations_grow_with_their_operand |
| COST029 | `bitAnd,bitOr,bitXor` | linear | operand bytes | `helper:bitwise_binop` | rholang/tests/cost_shapes.rs::linear_operations_grow_with_their_operand |
| COST030 | `bitNot,shiftLeft,shiftRight,shiftRightUnsigned` | linear | result bytes | `bitwise_cost,bitwise_int_cost,shift_left_result_size` | rholang/tests/cost_shapes.rs::linear_operations_grow_with_their_operand |
| COST031 | `Expr::ENot` | identity | - | `-` | rholang/tests/cost_shapes.rs::constant_operations_do_not_grow_with_the_operand |
| COST032 | `Expr::ENeg` | constant | - | `big_int_negation,decimal_negation` | rholang/tests/cost_shapes.rs::constant_operations_do_not_grow_with_the_operand |
| COST033 | `Expr::EMult,Expr::EDiv,Expr::EMod` | product | operand sizes | `big_int_multiplication,big_int_division,big_int_modulo,decimal_multiplication,decimal_division,multiplication_cost,division_cost,modulo_cost` | rholang/tests/cost_shapes.rs::product_operations_grow_with_both_operands |
| COST034 | `Expr::EPlus,Expr::EMinus` | linear | operand size, or the collection size for the collection arms | `op_call_cost,sum_cost,subtraction_cost,big_int_sum,big_int_subtraction,decimal_sum,decimal_subtraction,helper:set_add,helper:collection_delete` | rholang/tests/cost_shapes.rs::linear_operations_grow_with_their_operand |
| COST035 | `Expr::ELt,Expr::ELte,Expr::EGt,Expr::EGte` | linear | operand size (Int is fixed width) | `helper:relop,comparison_cost` | rholang/tests/cost_shapes.rs::constant_operations_do_not_grow_with_the_operand |
| COST036 | `Expr::EEq,Expr::ENeq` | linear | min serialized size | `equality_check_cost` | rholang/tests/cost_shapes.rs::linear_operations_grow_with_their_operand |
| COST037 | `Expr::EAnd,Expr::EOr,Expr::EShortAnd,Expr::EShortOr` | constant | - | `boolean_and_cost,boolean_or_cost,helper:bool_short_circuit` | rholang/tests/cost_shapes.rs::constant_operations_do_not_grow_with_the_operand |
| COST038 | `Expr::EMatches` | capped | C124's product cap | `-` | rholang/tests/cost_shapes.rs::constant_operations_do_not_grow_with_the_operand |
| COST039 | `Expr::EPercentPercent` | product | format x map size | `interpolate_cost,op_call_cost` | rholang/tests/cost_shapes.rs::product_operations_grow_with_both_operands |
| COST040 | `Expr::EPlusPlus` | linear | both operand counts | `list_append_cost,string_append_cost,union_cost,op_call_cost` | rholang/src/reduce.rs::a_union_charges_both_operands_alike |
| COST041 | `Expr::EMinusMinus` | product | both operand counts | `diff_cost,op_call_cost` | rholang/tests/cost_shapes.rs::product_operations_grow_with_both_operands |
| COST042 | `Expr::EVar` | constant | - | `var_eval_cost,helper:eval_var` | rholang/tests/cost_shapes.rs::constant_operations_do_not_grow_with_the_operand |
| COST043 | `Expr::EList,Expr::ETuple` | identity | - | `-` | rholang/tests/cost_shapes.rs::constant_operations_do_not_grow_with_the_operand |
| COST044 | `Expr::ESet,Expr::EMap` | identity | - | `-` | rholang/tests/cost_shapes.rs::constant_operations_do_not_grow_with_the_operand |
| COST045 | `Expr::EMethod` | constant | - | `method_call_cost` | rholang/tests/cost_shapes.rs::constant_operations_do_not_grow_with_the_operand |
| COST046 | `ChargingRSpace::produce` | linear | serialized size | `storage_cost_produce` | rholang/tests/cost_shapes.rs::linear_operations_grow_with_their_operand |
| COST047 | `ChargingRSpace::consume` | linear | serialized size | `storage_cost_consume` | rholang/tests/cost_shapes.rs::linear_operations_grow_with_their_operand |
| COST048 | `ChargingRSpace::produce_at,ChargingRSpace::consume_at,ChargingRSpace::commit_produce` | linear | channel count | `event_storage_cost,comm_event_storage_cost` | rholang/tests/cost_shapes.rs::linear_operations_grow_with_their_operand |
| COST049 | `ChargingRSpace::refund_storage` | linear | serialized size (negative) | `storage_cost_consume,storage_cost_produce` | rholang/tests/cost_shapes.rs::linear_operations_grow_with_their_operand |
| COST050 | `rho:gov:resolveWeights` | quadratic | member count | `gov_fold_cost` | rholang/tests/cost_shapes.rs::quadratic_operations_grow_with_the_square |
| COST051 | `rho:gov:trustLevels` | quadratic | member count | `gov_fold_cost` | rholang/tests/cost_shapes.rs::quadratic_operations_grow_with_the_square |
| COST052 | `rho:gov:censure` | cubic | member universe | `gov_censure_cost` | rholang/src/system_processes.rs::censure_is_charged_the_cube_of_its_universe |
| COST053 | `rho:gov:tally` | product | options x ballots | `gov_tally_ranked_cost,gov_tally_approval_cost` | rholang/src/system_processes.rs::a_tally_charges_the_option_lists_it_scans |
| COST054 | `parse` | linear | term length | `parsing_cost` | rholang/tests/cost_shapes.rs::linear_operations_grow_with_their_operand |
| COST055 | `substitution` | linear | serialized size | `helper:substitute_par_and_charge` | rholang/tests/cost_shapes.rs::linear_operations_grow_with_their_operand |

Rows whose `residual` is not `-`:

- **COST010** (log factor) — The scan is linear and the rebuild is n log n; the accepted residual.
- **COST015** (over-charge) — The work is O(1) and the charge is one phlo per element: an over-charge, which is the safe direction and is recorded rather than paid down.
- **COST016** (declared flat) — The bytes were charged at parse and again at storage, so the scan reads data already paid for: the September refutation, recorded rather than re-opened.
- **COST038** (not priced by phlo) — Bounded by the matcher's product cap (C124) rather than by a charge: capped work is not the class defect, which is unbounded work for a flat charge.
- **COST044** (declared flat) — Decision (149): the bytes were charged at parse and at storage, and the runtime route that builds a set is priced by `add` (C353); the literal route is bounded by the 64 KiB deploy term.
- **COST050** (measured) — 41 ms at the universe bound: the shape matches the charge; the constant is the table's one-per-step applied to a map lookup.
- **COST051** (measured) — The level cap bounds the rounds, so a 511-member chain converges in 0 ms at the bound.
