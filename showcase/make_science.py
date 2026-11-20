#!/usr/bin/env python3
"""Regenerates the science showcase corpus (stdlib only, deterministic seed).

Every file is a full-size, scrollable document from a different research domain.
Numeric tables come from documented models + seeded noise (no placeholders).
Run: python3 showcase/make_science.py
"""
import json, math, os, random

D = os.path.dirname(os.path.abspath(__file__))
R = random.Random(20261113)


def w(name, text):
    with open(os.path.join(D, name), "w") as f:
        f.write(text)


# ───────────────────────── 08.csv  climate station (4 years, daily) ─────────────────────────
rows = ["date,station,t_mean_c,t_min_c,t_max_c,precip_mm,pressure_hpa,rel_humidity_pct,wind_ms,solar_mj_m2"]
import datetime
d0 = datetime.date(2021, 1, 1)
for i in range(1461):
    d = d0 + datetime.timedelta(days=i)
    doy = d.timetuple().tm_yday
    season = math.sin(2 * math.pi * (doy - 110) / 365.25)
    t = 9.5 + 11.5 * season + R.gauss(0, 2.6) + 0.02 * i / 365
    rng = 6 + 3 * (1 - abs(season)) + R.random() * 2
    rain = max(0.0, R.gauss(0, 1) * 6 - 2.5) if R.random() < 0.38 else 0.0
    p = 1013 + R.gauss(0, 7) - 2.5 * (rain > 5)
    rh = min(100, max(25, 74 - 14 * season + 0.8 * rain + R.gauss(0, 7)))
    wind = max(0.2, R.gauss(3.6, 1.7) + 0.05 * rain)
    sol = max(0.5, 12 + 10 * season - 0.35 * rain + R.gauss(0, 1.6))
    rows.append(f"{d},DE-Potsdam-WMO10379,{t:.1f},{t - rng / 2:.1f},{t + rng / 2:.1f},{rain:.1f},{p:.1f},{rh:.0f},{wind:.1f},{sol:.1f}")
w("08.csv", "\n".join(rows) + "\n")

# ───────────────────────── 09.tsv  RNA-seq differential expression ─────────────────────────
genes = ("TP53 BRCA1 BRCA2 EGFR MYC KRAS PTEN RB1 CDKN2A ATM CHEK2 PIK3CA AKT1 MTOR VEGFA HIF1A IL6 TNF IFNG CD8A "
         "CD4 FOXP3 CTLA4 PDCD1 CD274 STAT3 JAK2 NFKB1 CASP3 BCL2 BAX MDM2 CCND1 CDK4 CDK6 E2F1 SOX2 NANOG POU5F1 "
         "GAPDH ACTB HMOX1 SOD2 NQO1 KEAP1 NFE2L2 SIRT1 FOXO3 PPARG ESR1 AR").split()
hdr = "gene_id\tsymbol\tbase_mean\tlog2_fold_change\tlfc_se\tstat\tpvalue\tpadj\tcond_a_mean\tcond_b_mean\tchrom\tbiotype"
trows = [hdr]
for i in range(1800):
    sym = genes[i] if i < len(genes) else f"{R.choice(['LINC','ZNF','SLC','KRT','MIR','CYP','OR','TMEM'])}{R.randint(1, 999)}"
    bm = 10 ** R.uniform(0.3, 4.6)
    lfc = R.gauss(0, 0.35) if R.random() < 0.82 else R.gauss(0, 2.1)
    se = max(0.05, 1.2 / math.sqrt(bm) + R.random() * 0.1)
    z = lfc / se
    p = max(1e-300, math.erfc(abs(z) / math.sqrt(2)))
    trows.append(f"ENSG{R.randint(100000, 299999):011d}\t{sym}\t{bm:.2f}\t{lfc:.3f}\t{se:.3f}\t{z:.3f}\t{p:.3e}\t{min(1.0, p * 1800 / (i + 1)):.3e}\t{bm * 2 ** (-lfc / 2):.1f}\t{bm * 2 ** (lfc / 2):.1f}\tchr{R.choice([str(c) for c in range(1, 23)] + ['X', 'Y'])}\t{R.choice(['protein_coding'] * 7 + ['lncRNA', 'miRNA', 'pseudogene'])}")
w("09.tsv", "\n".join(trows) + "\n")

# ───────────────────────── 18.vl  Vega-Lite: epidemic curve ─────────────────────────
S, I, Rr, N = 9_990_000.0, 10_000.0, 0.0, 1e7
beta, gamma = 0.31, 0.11
vals = []
for day in range(0, 240):
    new = beta * S * I / N
    rec = gamma * I
    S, I, Rr = S - new, I + new - rec, Rr + rec
    if day % 2 == 0:
        vals.append({"day": day, "compartment": "Susceptible", "people": round(S)})
        vals.append({"day": day, "compartment": "Infectious", "people": round(I)})
        vals.append({"day": day, "compartment": "Recovered", "people": round(Rr)})
w("18.vl", json.dumps({
    "$schema": "https://vega.github.io/schema/vega-lite/v5.json",
    "description": "SIR epidemic model, R0 = beta/gamma = 2.82, N = 10 M",
    "data": {"values": vals}, "mark": "line",
    "encoding": {"x": {"field": "day", "type": "quantitative"},
                 "y": {"field": "people", "type": "quantitative"},
                 "color": {"field": "compartment", "type": "nominal"}}}, indent=1) + "\n")

# ───────────────────────── 19.plotly  Michaelis–Menten with noise ─────────────────────────
vmax, km = 182.0, 3.4
xs = [round(0.1 * 1.07 ** i, 3) for i in range(70)]
obs = [round(vmax * x / (km + x) * (1 + R.gauss(0, 0.045)), 2) for x in xs]
fit = [round(vmax * x / (km + x), 2) for x in xs]
w("19.plotly", json.dumps({
    "data": [{"type": "scatter", "mode": "markers", "name": "initial rate (n=70)", "x": xs, "y": obs},
             {"type": "scatter", "mode": "lines", "name": "MM fit Vmax=182, Km=3.4 mM", "x": xs, "y": fit}],
    "layout": {"title": "Enzyme kinetics: hexokinase, 30 C", "xaxis": {"title": "[glucose] mM", "type": "log"},
               "yaxis": {"title": "v (umol/min/mg)"}}}, indent=1) + "\n")

# ───────────────────────── 06.dot  gene-regulatory network ─────────────────────────
nodes = ["TP53", "MDM2", "CDKN1A", "BAX", "PUMA", "ATM", "CHEK2", "MYC", "CCND1", "CDK4", "RB1", "E2F1", "PTEN",
         "PI3K", "AKT", "MTOR", "FOXO3", "BCL2", "CASP9", "CASP3", "NFKB1", "TNF", "IL6", "STAT3", "HIF1A", "VEGFA"]
edges = [("ATM", "CHEK2", "->"), ("ATM", "TP53", "->"), ("CHEK2", "TP53", "->"), ("TP53", "MDM2", "->"),
         ("MDM2", "TP53", "-|"), ("TP53", "CDKN1A", "->"), ("TP53", "BAX", "->"), ("TP53", "PUMA", "->"),
         ("PUMA", "BCL2", "-|"), ("BAX", "CASP9", "->"), ("CASP9", "CASP3", "->"), ("BCL2", "CASP9", "-|"),
         ("MYC", "CCND1", "->"), ("CCND1", "CDK4", "->"), ("CDK4", "RB1", "-|"), ("RB1", "E2F1", "-|"),
         ("E2F1", "MYC", "->"), ("CDKN1A", "CDK4", "-|"), ("PTEN", "PI3K", "-|"), ("PI3K", "AKT", "->"),
         ("AKT", "MTOR", "->"), ("AKT", "FOXO3", "-|"), ("AKT", "MDM2", "->"), ("FOXO3", "BAX", "->"),
         ("TNF", "NFKB1", "->"), ("NFKB1", "IL6", "->"), ("IL6", "STAT3", "->"), ("STAT3", "MYC", "->"),
         ("NFKB1", "BCL2", "->"), ("HIF1A", "VEGFA", "->"), ("MTOR", "HIF1A", "->"), ("STAT3", "VEGFA", "->")]
g = ["digraph GRN {", "  rankdir=LR;", '  node [shape=box];']
g += [f"  {n};" for n in nodes]
g += [f'  {a} -> {b}{" [arrowhead=tee]" if k == "-|" else ""};' for a, b, k in edges]
w("06.dot", "\n".join(g) + "\n}\n")

# ───────────────────────── 16.gv  central carbon metabolism ─────────────────────────
path = ["Glucose", "G6P", "F6P", "F16BP", "DHAP", "G3P", "BPG13", "PG3", "PG2", "PEP", "Pyruvate", "AcCoA", "Citrate",
        "Isocitrate", "aKG", "SuccCoA", "Succinate", "Fumarate", "Malate", "OAA"]
gv = ["digraph Metabolism {", "  rankdir=TB;", "  node [shape=ellipse];"]
for a, b in zip(path, path[1:] + ["Citrate"]):
    gv.append(f"  {a} -> {b};")
gv += ["  DHAP -> G3P;", "  G6P -> Ribose5P;", "  Ribose5P -> F6P;", "  Pyruvate -> Lactate;", "  Pyruvate -> OAA;"]
w("16.gv", "\n".join(gv) + "\n}\n")

# ───────────────────────── 05.mmd / 15.mermaid ─────────────────────────
w("05.mmd", """flowchart LR
    RAW[(Raw FASTQ<br/>2x150bp)] --> QC[FastQC / MultiQC]
    QC --> TRIM[fastp adapter trim]
    TRIM --> ALIGN[STAR 2-pass alignment]
    ALIGN --> SORT[samtools sort + index]
    SORT --> DEDUP[Picard MarkDuplicates]
    DEDUP --> COUNT[featureCounts]
    COUNT --> NORM[DESeq2 size factors]
    NORM --> DE{Differential expression}
    DE -->|padj < 0.05| GSEA[GSEA / MSigDB]
    DE -->|volcano| PLOT[ggplot2 figures]
    GSEA --> REPORT[Quarto report]
    PLOT --> REPORT
    DEDUP --> VAR[GATK HaplotypeCaller]
    VAR --> VQSR[VQSR filter]
    VQSR --> ANN[VEP annotation]
    ANN --> REPORT
""")
w("15.mermaid", """flowchart TD
    A[Experimental design] --> B[Data acquisition]
    B --> C{Missing values?}
    C -->|yes| D[Multiple imputation MICE]
    C -->|no| E[Feature scaling]
    D --> E
    E --> F[Train / validation / test split 70-15-15]
    F --> G[Hyperparameter search - Optuna TPE]
    G --> H[Gradient boosted trees]
    G --> I[Transformer encoder]
    G --> J[Gaussian process regression]
    H --> K[Ensemble stacking]
    I --> K
    J --> K
    K --> L{Calibration ECE < 0.03?}
    L -->|no| M[Isotonic recalibration]
    L -->|yes| N[SHAP explanations]
    M --> N
    N --> O[Report and model card]
""")

# ───────────────────────── 04.tex  quantum chemistry ─────────────────────────
w("04.tex", r"""% Hartree-Fock and post-HF methods
\documentclass{article}
\begin{document}
\section{The Electronic Schr\"odinger Equation}
Within the Born--Oppenheimer approximation the electronic Hamiltonian for $N$ electrons and $M$ nuclei is
\[ \hat H = -\sum_{i=1}^{N}\frac{1}{2}\nabla_i^2 - \sum_{i=1}^{N}\sum_{A=1}^{M}\frac{Z_A}{r_{iA}} + \sum_{i<j}\frac{1}{r_{ij}}. \]

\section{Hartree--Fock}
The Fock operator is $\hat f(1)=\hat h(1)+\sum_{j}\left[\hat J_j(1)-\hat K_j(1)\right]$ and the Roothaan--Hall equations read
\[ \mathbf{F}\mathbf{C}=\mathbf{S}\mathbf{C}\boldsymbol{\varepsilon}. \]
The Fock matrix elements in an atomic-orbital basis are
\[ F_{\mu\nu}=h_{\mu\nu}+\sum_{\lambda\sigma}P_{\lambda\sigma}\left[(\mu\nu|\sigma\lambda)-\tfrac{1}{2}(\mu\lambda|\sigma\nu)\right],\qquad P_{\lambda\sigma}=2\sum_{a}^{occ}C_{\lambda a}C_{\sigma a}. \]

\section{Correlation Energy}
M{\o}ller--Plesset second order perturbation theory gives
\[ E^{(2)}=\sum_{i<j}^{occ}\sum_{a<b}^{virt}\frac{|\langle ij||ab\rangle|^2}{\varepsilon_i+\varepsilon_j-\varepsilon_a-\varepsilon_b}. \]
Coupled cluster with singles and doubles uses the exponential ansatz
\[ |\Psi_{CC}\rangle=e^{\hat T}|\Phi_0\rangle,\qquad \hat T=\hat T_1+\hat T_2. \]

\section{Density Functional Theory}
Kohn--Sham equations: $\left[-\tfrac12\nabla^2+v_{\mathrm{eff}}(\mathbf r)\right]\phi_i=\varepsilon_i\phi_i$ with
\[ v_{\mathrm{eff}}=v_{\mathrm{ext}}+\int\frac{\rho(\mathbf r')}{|\mathbf r-\mathbf r'|}d\mathbf r'+\frac{\delta E_{xc}[\rho]}{\delta\rho}. \]
The local density exchange energy is
\[ E_x^{LDA}=-\frac{3}{4}\left(\frac{3}{\pi}\right)^{1/3}\int\rho^{4/3}(\mathbf r)\,d\mathbf r. \]

\section{Results: Water, cc-pVTZ}
The total HF energy is $E_{HF}=-76.0571\,E_h$, MP2 correlation $-0.2760\,E_h$, CCSD(T) correlation $-0.3000\,E_h$.
The dipole moment is $\mu=1.85\,\mathrm{D}$ and the O--H bond length $r_e=0.9572\,\mathrm{\AA}$.
\end{document}
""")

# ───────────────────────── 13.latex  fluid dynamics ─────────────────────────
w("13.latex", r"""\documentclass{article}
\begin{document}
\section{Incompressible Navier--Stokes}
\[ \rho\left(\frac{\partial \mathbf u}{\partial t}+\mathbf u\cdot\nabla\mathbf u\right)=-\nabla p+\mu\nabla^2\mathbf u+\mathbf f,\qquad \nabla\cdot\mathbf u=0. \]
The Reynolds number is $\mathrm{Re}=\dfrac{\rho U L}{\mu}$; laminar pipe flow becomes unstable near $\mathrm{Re}\approx 2300$.

\section{Turbulence}
Kolmogorov's $-5/3$ law for the inertial subrange:
\[ E(k)=C\,\varepsilon^{2/3}k^{-5/3},\qquad C\approx 1.5 \]
with the Kolmogorov microscale $\eta=\left(\nu^3/\varepsilon\right)^{1/4}$.
Reynolds-averaged equations introduce the stress tensor $-\rho\overline{u_i'u_j'}$ closed by the $k$--$\varepsilon$ model:
\[ \nu_t=C_\mu\frac{k^2}{\varepsilon},\quad \frac{Dk}{Dt}=P_k-\varepsilon+\nabla\cdot\!\left[\left(\nu+\frac{\nu_t}{\sigma_k}\right)\nabla k\right]. \]

\section{Boundary Layer}
Blasius solution: $f'''+\tfrac12 ff''=0$, $f(0)=f'(0)=0$, $f'(\infty)=1$, giving $\delta_{99}\approx 4.91\sqrt{\nu x/U}$ and
\[ C_f=\frac{0.664}{\sqrt{\mathrm{Re}_x}}. \]

\section{Numerical Method}
Finite volume discretisation with the SIMPLE algorithm; the pressure correction satisfies
\[ \nabla\cdot\left(\frac{1}{a_P}\nabla p'\right)=\nabla\cdot\mathbf u^{*}. \]
CFL stability: $\mathrm{CFL}=\dfrac{u\,\Delta t}{\Delta x}\le 1$ for explicit schemes.
\end{document}
""")

# ───────────────────────── 14.typ  materials science ─────────────────────────
w("14.typ", r"""= Density Functional Study of Perovskite Solar Absorbers

== Band structure

The Shockley--Queisser detailed-balance limit depends on the bandgap $E_g$:
$ eta_"SQ" = (J_"sc" V_"oc" "FF") / P_"in" $
For $"CH"_3"NH"_3"PbI"_3$ we obtain $E_g = 1.55 "eV"$ and $eta_"SQ" = 31.4 %$.

== Defect formation

The formation energy of a charged defect $D^q$:
$ E_f[D^q] = E_"tot"[D^q] - E_"tot"["bulk"] - sum_i n_i mu_i + q (E_"VBM" + E_F) + E_"corr" $

== Effective mass

Parabolic band: $ 1 / m^* = 1 / hbar^2 (diff^2 E) / (diff k^2) $

== Absorption

$ alpha(omega) = (omega) / (n c) epsilon_2(omega), quad epsilon_2 = (4 pi^2 e^2) / (Omega m^2 omega^2) sum |p_(c v)|^2 delta(E_c - E_v - hbar omega) $

#table(
  columns: 4,
  [Material], [$E_g$ (eV)], [$mu_e$ (cm²/Vs)], [Stability],
  [MAPbI3], [1.55], [66], [low],
  [FAPbI3], [1.48], [80], [medium],
  [CsPbBr3], [2.30], [45], [high],
  [Cs2AgBiBr6], [2.05], [11], [high],
  [CdTe], [1.50], [1100], [high],
  [Si], [1.12], [1400], [high],
)
""")

# ───────────────────────── 01.md  clinical trial survival ─────────────────────────
tbl = ["| Month | At risk A | At risk B | S(t) A | S(t) B |", "|---:|---:|---:|---:|---:|"]
nA = nB = 412
sA = sB = 1.0
for m in range(0, 61, 3):
    tbl.append(f"| {m} | {nA} | {nB} | {sA:.3f} | {sB:.3f} |")
    dA = int(nA * (1 - math.exp(-0.021 * 3)))
    dB = int(nB * (1 - math.exp(-0.014 * 3)))
    sA *= 1 - dA / max(nA, 1)
    sB *= 1 - dB / max(nB, 1)
    nA -= dA + int(nA * 0.01)
    nB -= dB + int(nB * 0.01)
md = r"""# Phase III Randomised Trial: Adjuvant Immunotherapy vs Standard of Care

*Primary endpoint: overall survival (OS). Secondary: progression-free survival, objective response, grade 3+ toxicity.*

## 1. Design

824 patients were randomised 1:1 (stratified by stage, ECOG, PD-L1 status). The sample size gives 90% power to detect a hazard ratio $\mathrm{HR}=0.75$ at two-sided $\alpha=0.05$ with
$$ D=\frac{4\,(z_{1-\alpha/2}+z_{1-\beta})^2}{\ln^2(\mathrm{HR})}\approx 509 \text{ events}. $$

## 2. Survival model

The Kaplan--Meier estimator is $\hat S(t)=\prod_{t_i\le t}\left(1-\dfrac{d_i}{n_i}\right)$ with Greenwood variance
$$ \widehat{\mathrm{Var}}[\hat S(t)]=\hat S(t)^2\sum_{t_i\le t}\frac{d_i}{n_i(n_i-d_i)}. $$
The Cox proportional hazards model: $h(t\mid x)=h_0(t)\exp(\beta^\top x)$ with partial likelihood
$$ L(\beta)=\prod_{i:\,\delta_i=1}\frac{\exp(\beta^\top x_i)}{\sum_{j\in R(t_i)}\exp(\beta^\top x_j)}. $$

## 3. Life table

""" + "\n".join(tbl) + r"""

## 4. Cox regression

| Covariate | HR | 95% CI | p |
|:---|---:|:---:|---:|
| Treatment (B vs A) | 0.71 | 0.59-0.86 | <0.001 |
| Age (per 10 y) | 1.18 | 1.06-1.31 | 0.002 |
| ECOG 1 vs 0 | 1.42 | 1.15-1.76 | 0.001 |
| Stage IV vs III | 1.95 | 1.58-2.41 | <0.001 |
| PD-L1 >=50% | 0.64 | 0.50-0.82 | <0.001 |
| Smoking (current) | 1.27 | 1.02-1.58 | 0.031 |

Median OS: arm A 31.4 months, arm B 44.9 months. The Schoenfeld residual test showed no violation of proportionality ($\chi^2=3.2$, $p=0.36$).

## 5. Analysis code

```python
from lifelines import KaplanMeierFitter, CoxPHFitter
kmf = KaplanMeierFitter()
kmf.fit(df.loc[df.arm == "B", "months"], df.loc[df.arm == "B", "event"], label="B")
cph = CoxPHFitter().fit(df, duration_col="months", event_col="event",
                        formula="arm + age + ecog + stage + pdl1")
cph.print_summary()
```

## 6. Bayesian sensitivity

With a prior $\log\mathrm{HR}\sim\mathcal N(0,\,0.35^2)$ the posterior is $\log\mathrm{HR}\mid D\sim\mathcal N(-0.331,\,0.098^2)$, so $P(\mathrm{HR}<1\mid D)>0.999$ and $P(\mathrm{HR}<0.8\mid D)=0.91$.
"""
w("01.md", md)

# ───────────────────────── 02.qmd  SEIR epidemiology ─────────────────────────
w("02.qmd", r"""---
title: "Stochastic SEIR Modelling of Seasonal Influenza"
author: "Computational Epidemiology Group"
format: html
---

## Compartmental model

$$
\frac{dS}{dt}=-\beta\frac{SI}{N},\quad
\frac{dE}{dt}=\beta\frac{SI}{N}-\sigma E,\quad
\frac{dI}{dt}=\sigma E-\gamma I,\quad
\frac{dR}{dt}=\gamma I
$$

The basic reproduction number is $R_0=\beta/\gamma$ and the next-generation matrix approach gives the dominant eigenvalue
$$ R_0=\rho\!\left(FV^{-1}\right)=\frac{\beta\sigma}{\sigma\gamma}. $$

```{python}
import numpy as np
from scipy.integrate import solve_ivp

def seir(t, y, beta, sigma, gamma, N):
    S, E, I, R = y
    return [-beta*S*I/N, beta*S*I/N - sigma*E, sigma*E - gamma*I, gamma*I]

sol = solve_ivp(seir, (0, 200), [9.99e6, 5e3, 5e3, 0],
                args=(0.45, 1/1.9, 1/3.2, 1e7), dense_output=True)
```

## Parameter estimates (MCMC, 4 chains x 2000)

| Parameter | Mean | 95% CrI | R-hat |
|:---|---:|:---:|---:|
| $\beta$ | 0.452 | 0.431-0.474 | 1.001 |
| $1/\sigma$ (days) | 1.91 | 1.62-2.24 | 1.002 |
| $1/\gamma$ (days) | 3.18 | 2.84-3.55 | 1.001 |
| $R_0$ | 1.42 | 1.36-1.49 | 1.001 |
| Reporting rate | 0.31 | 0.27-0.36 | 1.003 |

The final attack rate solves $z=1-e^{-R_0 z}$, giving $z\approx0.55$ for $R_0=1.42$.

## Intervention scenarios

With vaccination coverage $v$ and efficacy $\varepsilon$ the effective reproduction number is $R_e=R_0\,(1-v\varepsilon)$, so the herd immunity threshold is
$$ v_c=\frac{1}{\varepsilon}\left(1-\frac{1}{R_0}\right)\approx 0.34. $$

```{python}
for cov in (0.0, 0.2, 0.4, 0.6):
    print(cov, 1.42 * (1 - cov * 0.6))
```
""")

# ───────────────────────── 11.mdx  astrophysics ─────────────────────────
w("11.mdx", r"""import { LightCurve } from '../components/LightCurve'

# Exoplanet Transit Photometry: TOI-1452 b

## Transit depth

The fractional flux drop for a planet of radius $R_p$ around a star of radius $R_\star$:
$$ \delta=\left(\frac{R_p}{R_\star}\right)^2=\frac{\Delta F}{F}\approx 0.00412 $$
giving $R_p=1.67\,R_\oplus$ for $R_\star=0.275\,R_\odot$.

## Orbital mechanics

Kepler's third law: $a^3=\dfrac{G M_\star P^2}{4\pi^2}$, with $P=11.06$ d and $M_\star=0.249\,M_\odot$ giving $a=0.061$ AU.
The equilibrium temperature $T_{eq}=T_\star\sqrt{\dfrac{R_\star}{2a}}\,\left[f(1-A_B)\right]^{1/4}=326$ K.

## Mass from radial velocity

$$ K=\left(\frac{2\pi G}{P}\right)^{1/3}\frac{M_p\sin i}{(M_\star+M_p)^{2/3}}\frac{1}{\sqrt{1-e^2}} $$

| Parameter | Value | Unit |
|:---|---:|:---|
| Period | 11.0620 | d |
| T0 | 2459000.1234 | BJD |
| Rp/R* | 0.0642 | |
| Impact parameter b | 0.23 | |
| Inclination | 89.2 | deg |
| Planet mass | 4.82 | M_earth |
| Bulk density | 5.6 | g/cm3 |

## Light-curve fit

Limb darkening (quadratic): $I(\mu)/I_0=1-u_1(1-\mu)-u_2(1-\mu)^2$ with $u_1=0.31$, $u_2=0.28$. The Gaussian-process noise kernel:
$$ k(\tau)=\sigma^2\left(1+\frac{\sqrt3\,\tau}{\ell}\right)\exp\!\left(-\frac{\sqrt3\,\tau}{\ell}\right). $$

<LightCurve target="TOI-1452" sector={17} />
""")

# ───────────────────────── 12.rmd  ecology ─────────────────────────
w("12.rmd", r"""---
title: "Linear Mixed Models for Biodiversity Gradients"
output: html_document
---

## Species richness along elevation

We model richness $y_{ij}$ at site $i$ in region $j$:
$$ y_{ij}=\beta_0+\beta_1\,\mathrm{elev}_{ij}+\beta_2\,\mathrm{elev}_{ij}^2+u_j+\varepsilon_{ij},\quad u_j\sim\mathcal N(0,\sigma_u^2),\ \varepsilon_{ij}\sim\mathcal N(0,\sigma^2). $$

```{r}
library(lme4)
m <- lmer(richness ~ elev + I(elev^2) + temp + (1 | region), data = df)
summary(m)
```

The intraclass correlation is $\mathrm{ICC}=\dfrac{\sigma_u^2}{\sigma_u^2+\sigma^2}=0.34$.

## Diversity indices

Shannon: $H'=-\sum_{i=1}^S p_i\ln p_i$; Simpson: $D=1-\sum p_i^2$; Hill numbers
$$ {}^qD=\left(\sum_{i=1}^S p_i^q\right)^{1/(1-q)}. $$

| Elevation (m) | Sites | Richness | Shannon H' | Simpson |
|---:|---:|---:|---:|---:|
| 200 | 24 | 48.3 | 3.41 | 0.94 |
| 600 | 24 | 61.7 | 3.72 | 0.96 |
| 1000 | 24 | 66.2 | 3.85 | 0.96 |
| 1400 | 24 | 52.8 | 3.51 | 0.93 |
| 1800 | 24 | 37.1 | 3.02 | 0.89 |
| 2200 | 24 | 21.4 | 2.44 | 0.82 |
| 2600 | 24 | 11.9 | 1.81 | 0.71 |

Peak richness occurs at $-\beta_1/(2\beta_2)=1013$ m (mid-elevation peak, consistent with the mid-domain effect).
""")

print("generated science showcase")
