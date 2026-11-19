use ii::{chem::transpile, heal::State};

#[test]
fn heal_balances_nested_latex_and_dangling_scripts() {
    let mut st = State::new();
    let mut out = Vec::new();

    st.heal(b"\\begin{align*}\\left[\\frac{a}{b}_", &mut out);
    assert_eq!(
        out,
        b"\\begin{align*}\\left[\\frac{a}{b}_{}\\right.\\end{align*}"
    );

    out.clear();
    st.heal(b"x^", &mut out);
    assert_eq!(out, b"x^{}");
}

#[test]
fn chem_transpiles_reversible_reactions_and_charges() {
    let mut out = Vec::new();
    transpile(b"Fe3+ + e- <=> Fe2+", &mut out);
    assert_eq!(
        out,
        b"\\text{Fe}^{3+} + \\text{e}^{-} \\rightleftharpoons \\text{Fe}^{2+}"
    );
}
