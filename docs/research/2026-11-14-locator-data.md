# Locator research: the data tables (2026-11-14)

> Beside [`2026-11-14-locator.md`](2026-11-14-locator.md), §3. Measured on this Mac (M4, 16 GB) on 2026-11-14, under load from other agents' builds, so no time here is a benchmark. The corpus is bench100 (`distillPDF/benchmarking/bench100`, the 141 PDFs of `docs/reports/corpus-2026-11-08.md`). Tools: pikepdf 10.7.3 (qpdf's library), qpdf 12.4.2, mutool (MuPDF 1.27), PDFKit on macOS through a three-line Swift script (`PDFDocument(url:)` then `write(to:)`: the framework Preview is built on; that a Preview save takes exactly this path is not verified), and RectoPDF's own `recto local FILE text --page N --json` from the dev build in `app/build.noindex`, in a quiet run's throwaway state. The scripts were scratch files outside the repo and are gone; what each measured is said with each table.

## A. Every file: identity and structure (141 files)

`/ID`: the trailer has one. `ID[0]=ID[1]`: both strings the same, a file never updated or written so. Outline entries: every entry, at every depth. Named dests: the old `/Dests` dictionary and the `/Names` `/Dests` tree together. Structure tree: the catalog has `/StructTreeRoot`. Pages with `/StructParents`: pages whose content is tied into it. Sample names: the first two names, cut at 28 characters.

| File | Pages | /ID | ID[0]=ID[1] | Outline entries | Named dests | Structure tree | Pages with /StructParents | /PageLabels | Sample names |
|---|---:|---|---|---:|---:|---|---:|---|---|
| astro-ph_GA_2607.28278 | 9 | yes | no | 7 | 69 | no | 0 | no | `Doc-Start`, `Hfootnote.2` |
| astro-ph_GA_2607.28501 | 17 | yes | no | 16 | 177 | no | 0 | yes | `Doc-Start`, `Item.1` |
| astro-ph_GA_2607.28517 | 18 | yes | no | 14 | 146 | no | 0 | yes | `Doc-Start`, `Hfootnote.1` |
| astro-ph_GA_2607.28564 | 73 | yes | no | 42 | 977 | no | 0 | yes | `Item.1`, `Item.10` |
| astro-ph_GA_2607.28594 | 27 | yes | no | 22 | 198 | no | 0 | yes | `Doc-Start`, `Hfootnote.1` |
| cs_CL_2607.28528 | 13 | yes | yes | 0 | 0 | yes | 13 | no |  |
| cs_CL_2607.28545 | 33 | yes | no | 32 | 274 | no | 0 | no | `Doc-Start`, `Hfootnote.1` |
| cs_CL_2607.28568 | 61 | yes | no | 48 | 248 | no | 0 | no | `Doc-Start`, `Hfootnote.1` |
| cs_CL_2607.28576 | 29 | yes | no | 37 | 159 | no | 0 | no | `Doc-Start`, `Item.1` |
| cs_CL_2607.28590 | 19 | yes | no | 27 | 121 | no | 0 | no | `Doc-Start`, `Hfootnote.1` |
| cs_CL_2607.28591 | 15 | yes | no | 0 | 0 | no | 0 | no |  |
| cs_CL_2607.28607 | 36 | yes | no | 0 | 127 | no | 0 | no | `Doc-Start`, `Hfootnote.1` |
| cs_CL_2607.28609 | 51 | yes | no | 28 | 224 | no | 0 | no | `Doc-Start`, `Hfootnote.1` |
| cs_CL_2607.28617 | 24 | yes | no | 21 | 118 | no | 0 | no | `Doc-Start`, `Hfootnote.1` |
| cs_CL_2607.28618 | 10 | yes | no | 12 | 90 | no | 0 | no | `Doc-Start`, `Item.1` |
| cs_CV_2607.28589 | 13 | yes | no | 20 | 105 | no | 0 | no | `Doc-Start`, `Hfootnote.1` |
| cs_CV_2607.28595 | 34 | yes | no | 41 | 430 | no | 0 | no | `Doc-Start`, `appendix.A` |
| cs_CV_2607.28611 | 40 | yes | no | 25 | 232 | no | 0 | no | `Doc-Start`, `Hfootnote.1` |
| cs_CV_2607.28624 | 25 | yes | no | 29 | 228 | no | 0 | no | `Doc-Start`, `Hfootnote.1` |
| cs_CV_2607.28625 | 36 | yes | no | 34 | 244 | no | 0 | no | `Doc-Start`, `cite.DBLP:conf/cvpr/Shin0HB2` |
| cs_CV_2607.28627 | 18 | yes | no | 22 | 120 | no | 0 | no | `Doc-Start`, `Hfootnote.1` |
| cs_DS_2607.28260 | 46 | yes | no | 23 | 284 | no | 0 | no | `AlgoLine.1.1`, `AlgoLine.1.2` |
| cs_DS_2607.28298 | 36 | yes | no | 25 | 167 | no | 0 | no | `AMS.10`, `AMS.6` |
| cs_DS_2607.28574 | 20 | yes | no | 18 | 129 | no | 0 | no | `Doc-Start`, `Hfootnote.1` |
| cs_DS_2607.28575 | 18 | yes | no | 9 | 100 | no | 0 | no | `AMS.8`, `Doc-Start` |
| cs_DS_2607.28610 | 45 | yes | no | 20 | 314 | no | 0 | no | `ALG@line.1.1`, `ALG@line.1.10` |
| cs_SE_2607.28331 | 7 | yes | no | 12 | 51 | no | 0 | no | `Doc-Start`, `Item.1` |
| cs_SE_2607.28424 | 17 | yes | no | 23 | 169 | no | 0 | no | `Doc-Start`, `Hfootnote.1` |
| cs_SE_2607.28431 | 14 | yes | no | 15 | 80 | no | 0 | no | `Doc-Start`, `Hfootnote.1` |
| cs_SE_2607.28529 | 9 | yes | no | 0 | 0 | no | 0 | no |  |
| cs_SE_2607.28587 | 13 | yes | no | 24 | 123 | no | 0 | no | `Doc-Start`, `cite.aleithan_swe-bench_2024` |
| econ_EM_2607.27485 | 35 | yes | yes | 0 | 0 | yes | 35 | no |  |
| econ_EM_2607.27505 | 52 | yes | no | 0 | 222 | no | 0 | no | `Doc-Start`, `Hfootnote.1` |
| econ_EM_2607.28131 | 35 | yes | no | 16 | 141 | no | 0 | no | `Doc-Start`, `Hfootnote.1` |
| econ_EM_2607.28291 | 45 | yes | no | 17 | 193 | no | 0 | no | `Doc-Start`, `Hfootnote.1` |
| econ_EM_2607.28294 | 33 | yes | no | 0 | 0 | no | 0 | no |  |
| eess_SP_2607.28115 | 28 | yes | no | 23 | 157 | no | 0 | no | `Doc-Start`, `Item.1` |
| eess_SP_2607.28201 | 5 | yes | no | 0 | 0 | no | 0 | no |  |
| eess_SP_2607.28224 | 13 | yes | no | 0 | 0 | no | 0 | no |  |
| eess_SP_2607.28395 | 13 | yes | no | 16 | 101 | no | 0 | no | `Doc-Start`, `Hfootnote.1` |
| eess_SP_2607.28404 | 12 | yes | no | 12 | 68 | no | 0 | no | `Doc-Start`, `cite.bek2015_cairo_postcorr` |
| eu_law_eu_aiact | 144 | yes | no | 14 | 132 | yes | 144 | no | `mark_1`, `mark_10` |
| eu_law_eu_dma | 66 | yes | yes | 2 | 50 | yes | 66 | yes | `BK_L_20220914-013EN001_ACT`, `BK_L_20220914-013EN002_ANNEX` |
| eu_law_eu_dsa | 102 | yes | yes | 1 | 83 | yes | 102 | yes | `BK_L_20221017-018EN001_ACT`, `BK_NOTE_E0001-001_ACT` |
| eu_law_eu_gdpr | 88 | yes | no | 1 | 1 | yes | 88 | no | `bookmark_nopage_001` |
| eu_law_nonenglish_eu_gdpr_de | 88 | yes | no | 1 | 1 | yes | 88 | no | `bookmark_nopage_001` |
| eu_law_nonenglish_eu_gdpr_fr | 88 | yes | no | 1 | 1 | yes | 88 | no | `bookmark_nopage_001` |
| forms_irs_f1040 | 2 | yes | no | 0 | 0 | yes | 2 | no |  |
| forms_irs_f1040s1 | 2 | yes | no | 0 | 0 | yes | 2 | no |  |
| forms_irs_f1040sc | 2 | yes | no | 0 | 0 | yes | 2 | no |  |
| forms_irs_f1065 | 6 | yes | no | 0 | 0 | yes | 6 | no |  |
| forms_irs_f1099msc | 6 | yes | no | 0 | 0 | yes | 6 | no |  |
| forms_irs_f1120 | 6 | yes | no | 0 | 0 | yes | 6 | no |  |
| forms_irs_f2848 | 2 | yes | no | 0 | 0 | yes | 2 | no |  |
| forms_irs_f4562 | 3 | yes | no | 0 | 0 | yes | 3 | no |  |
| forms_irs_f8949 | 2 | yes | no | 0 | 0 | yes | 2 | no |  |
| forms_irs_f941 | 3 | yes | no | 0 | 0 | yes | 3 | no |  |
| forms_irs_f990 | 12 | yes | no | 0 | 0 | yes | 12 | no |  |
| gov_nasa_ntrs20150023275 | 182 | yes | no | 0 | 35 | no | 0 | no | `PCOS_2015_PATR_093015.indd:.`, `PCOS_2015_PATR_093015.indd:.` |
| gov_nasa_ntrs20160011963 | 18 | yes | no | 0 | 0 | no | 0 | no |  |
| gov_nasa_ntrs20180005693 | 152 | yes | no | 74 | 136 | yes | 143 | yes | `2017 R&T Report.indd:.36792:`, `2017 R&T Report.indd:.36794:` |
| gov_nasa_ntrs20190025705 | 127 | yes | no | 0 | 0 | yes | 127 | no |  |
| gov_nasa_ntrs20205000464 | 36 | yes | yes | 0 | 0 | yes | 36 | no |  |
| gov_nasa_ntrs20210011071 | 120 | yes | no | 57 | 165 | no | 0 | yes | `2020 R&T Report.indd:.52254:`, `2020 R&T Report.indd:.52260:` |
| gov_nasa_ntrs20210023723 | 50 | yes | yes | 0 | 0 | yes | 50 | no |  |
| gov_nasa_ntrs20220005969 | 196 | yes | no | 91 | 243 | yes | 193 | yes | `2021 R&T (2109) 2022_0419.in`, `2021 R&T (2109) 2022_0419.in` |
| gov_nasa_ntrs20230009525 | 85 | yes | yes | 0 | 0 | yes | 85 | no |  |
| gov_nasa_ntrs20240002628 | 383 | yes | no | 228 | 0 | yes | 28 | yes |  |
| gov_usgs_usgs70275236 | 54 | yes | no | 54 | 191 | yes | 52 | yes | `sir20265125.indd:Abstract:41`, `sir20265125.indd:Acknowledgm` |
| gov_usgs_usgs70277269 | 32 | yes | no | 71 | 128 | yes | 29 | yes | `sir20265029_508.indd:.34743:`, `sir20265029_508.indd:.34764:` |
| gov_usgs_usgs70277338 | 1 | yes | no | 0 | 0 | no | 0 | no |  |
| gov_usgs_usgs70277647 | 2 | yes | no | 1 | 13 | yes | 2 | no | `res25-0053_Wi_3DEP_FS.indd:.`, `res25-0053_Wi_3DEP_FS.indd:.` |
| gov_usgs_usgs70277728 | 29 | yes | no | 21 | 115 | yes | 29 | yes | `Orndorff_StratNotes_PP1879-3`, `Orndorff_StratNotes_PP1879-3` |
| gov_usgs_usgs70277796 | 2 | yes | no | 10 | 13 | yes | 2 | no | `fs2026-3016.indd:Accessing t`, `fs2026-3016.indd:Acknowledgm` |
| gov_usgs_usgs70277797 | 26 | yes | no | 19 | 100 | yes | 25 | yes | `cir1570.indd:Abstract:375`, `cir1570.indd:Acknowledgments` |
| gov_usgs_usgs70277798 | 64 | yes | no | 51 | 285 | yes | 63 | yes | `sir20265043.indd:10. Boxplot`, `sir20265043.indd:11. Plots s` |
| gov_usgs_usgs70278109 | 4 | yes | no | 0 | 17 | yes | 4 | no | `den26-0031_for_layout.indd:A`, `den26-0031_for_layout.indd:A` |
| gov_usgs_usgs70278110 | 4 | yes | no | 0 | 29 | yes | 4 | no | `den26-0030_for_layout.indd:A`, `den26-0030_for_layout.indd:A` |
| gov_usgs_usgs70278165 | 98 | yes | no | 73 | 321 | yes | 98 | yes | `tm11c11.indd:.63754:804`, `tm11c11.indd:.63756:805` |
| math_AG_2607.28205 | 40 | yes | no | 25 | 146 | no | 0 | no | `Doc-Start`, `appendix.A` |
| math_AG_2607.28288 | 22 | yes | no | 12 | 186 | no | 0 | no | `Doc-Start`, `Item.1` |
| math_AG_2607.28368 | 22 | yes | no | 11 | 133 | no | 0 | no | `ALG@line.1`, `ALG@line.2` |
| math_AG_2607.28411 | 83 | yes | no | 17 | 555 | no | 0 | no | `Doc-Start`, `Hfootnote.1` |
| math_AG_2607.28586 | 50 | yes | no | 23 | 320 | no | 0 | no | `AMS.2`, `Doc-Start` |
| math_NA_2607.28104 | 41 | yes | no | 30 | 203 | no | 0 | no | `Doc-Start`, `algorithm.1` |
| math_NA_2607.28235 | 45 | yes | no | 18 | 207 | no | 0 | no | `Doc-Start`, `Item.1` |
| math_NA_2607.28286 | 24 | yes | no | 0 | 0 | no | 0 | no |  |
| math_NA_2607.28409 | 25 | yes | no | 16 | 116 | no | 0 | no | `Doc-Start`, `appendix.A` |
| math_NA_2607.28456 | 12 | yes | no | 27 | 135 | no | 0 | no | `Doc-Start`, `Hfootnote.1` |
| math_PR_2607.28413 | 24 | yes | no | 17 | 165 | no | 0 | no | `ALC@unique.1`, `ALC@unique.10` |
| math_PR_2607.28518 | 28 | yes | no | 9 | 232 | no | 0 | no | `AMS.9`, `Doc-Start` |
| math_PR_2607.28566 | 16 | yes | no | 7 | 83 | no | 0 | no | `Doc-Start`, `Item.1` |
| math_PR_2607.28578 | 55 | yes | no | 89 | 469 | no | 0 | no | `Doc-Start`, `Item.1` |
| math_PR_2607.28614 | 33 | yes | no | 13 | 249 | no | 0 | no | `Doc-Start`, `Item.1` |
| physics_optics_2607.28339 | 10 | yes | no | 15 | 60 | no | 0 | no | `Doc-Start`, `cite.Baskin2002_UltrafastDyn` |
| physics_optics_2607.28388 | 7 | yes | no | 3 | 47 | no | 0 | yes | `cite.Eilbeck1985`, `cite.FCC_2202` |
| physics_optics_2607.28482 | 42 | yes | no | 21 | 0 | yes | 42 | no |  |
| physics_optics_2607.28558 | 12 | yes | no | 0 | 0 | no | 0 | no |  |
| physics_optics_2607.28620 | 8 | yes | no | 8 | 102 | no | 0 | yes | `cite.Aharonov1987`, `cite.Alessandrini2025` |
| policy_worldbank_wbD34442276 | 10 | yes | yes | 0 | 0 | yes | 10 | no |  |
| policy_worldbank_wbD34442277 | 20 | yes | yes | 0 | 0 | yes | 20 | no |  |
| policy_worldbank_wbD34442284 | 21 | yes | yes | 0 | 0 | yes | 21 | no |  |
| policy_worldbank_wbD34442285 | 23 | yes | yes | 0 | 0 | yes | 23 | no |  |
| policy_worldbank_wbD34466043 | 9 | yes | yes | 0 | 0 | no | 0 | no |  |
| policy_worldbank_wbD34466164 | 10 | yes | yes | 0 | 0 | no | 0 | no |  |
| policy_worldbank_wbD34466172 | 14 | yes | yes | 0 | 0 | no | 0 | no |  |
| policy_worldbank_wbD34466295 | 19 | yes | yes | 0 | 0 | no | 0 | no |  |
| policy_worldbank_wbD34466311 | 19 | yes | yes | 0 | 0 | no | 0 | no |  |
| policy_worldbank_wbD34466334 | 9 | yes | yes | 0 | 0 | no | 0 | no |  |
| policy_worldbank_wbD34466335 | 9 | yes | yes | 0 | 0 | no | 0 | no |  |
| policy_worldbank_wbD34466337 | 9 | yes | yes | 0 | 0 | no | 0 | no |  |
| q-bio_NC_2607.26322 | 7 | yes | no | 11 | 61 | no | 0 | no | `Doc-Start`, `Hfootnote.1` |
| q-bio_NC_2607.26666 | 49 | yes | no | 0 | 0 | yes | 44 | no |  |
| q-bio_NC_2607.27308 | 15 | yes | no | 14 | 88 | no | 0 | no | `Doc-Start`, `appendix.Appendix.` |
| q-bio_NC_2607.28068 | 18 | yes | no | 32 | 127 | no | 0 | no | `Doc-Start`, `cite.AlamElDin2025` |
| q-bio_NC_2607.28601 | 35 | yes | no | 16 | 136 | no | 0 | no | `Doc-Start`, `algorithm.1` |
| quant-ph_2607.28598 | 22 | yes | no | 8 | 80 | no | 0 | no | `Doc-Start`, `Item.1` |
| quant-ph_2607.28600 | 28 | yes | no | 16 | 187 | no | 0 | no | `AMS.30`, `Doc-Start` |
| quant-ph_2607.28602 | 32 | yes | no | 6 | 226 | no | 0 | no | `Doc-Start`, `Hfootnote.1` |
| quant-ph_2607.28605 | 63 | yes | no | 45 | 763 | no | 0 | yes | `Item.1`, `Item.10` |
| quant-ph_2607.28621 | 25 | yes | no | 27 | 322 | no | 0 | yes | `AlgoLine.1.1`, `AlgoLine.1.10` |
| standards_ietf_rfc9000 | 151 | yes | no | 219 | 3022 | no | 151 | no | `AEAD`, `ALPN` |
| standards_ietf_rfc9001 | 52 | yes | no | 83 | 945 | no | 51 | no | `AEAD`, `AEBounds` |
| standards_ietf_rfc9002 | 42 | yes | no | 82 | 808 | no | 38 | no | `FACK`, `PRR` |
| standards_ietf_rfc9110 | 194 | yes | no | 311 | 5345 | no | 190 | no | `ALTSVC`, `BCP13` |
| standards_ietf_rfc9111 | 35 | yes | no | 76 | 1004 | no | 35 | no | `COOKIE`, `HTTP` |
| standards_ietf_rfc9112 | 46 | yes | no | 83 | 1232 | no | 45 | no | `CACHING`, `HTTP` |
| standards_ietf_rfc9113 | 78 | yes | no | 106 | 1654 | no | 72 | no | `BREACH`, `BadCipherSuites` |
| standards_ietf_rfc9114 | 57 | yes | no | 96 | 1760 | no | 57 | no | `ALTSVC`, `BREACH` |
| standards_ietf_rfc9205 | 27 | yes | no | 49 | 526 | no | 27 | no | `BCP190`, `COOKIES` |
| standards_ietf_rfc9260 | 133 | yes | no | 180 | 2983 | no | 125 | no | `ALLMAN99`, `FALL96` |
| standards_ietf_rfc9293 | 98 | yes | no | 91 | 1709 | no | 0 | no | `DS78`, `Err1283` |
| standards_ietf_rfc9420 | 132 | yes | no | 133 | 2029 | no | 0 | no | `ART`, `CLINIC` |
| standards_ietf_rfc9457 | 16 | yes | no | 31 | 287 | no | 0 | no | `HTML5`, `I-D.bhutton-json-schema` |
| standards_ietf_rfc9562 | 46 | yes | no | 59 | 874 | no | 0 | no | `Acknowledgements`, `Background` |
| stat_ML_2607.28248 | 27 | yes | no | 60 | 310 | no | 0 | no | `Doc-Start`, `Item.1` |
| stat_ML_2607.28322 | 94 | yes | no | 38 | 386 | no | 0 | no | `Doc-Start`, `Hfootnote.1` |
| stat_ML_2607.28338 | 20 | yes | no | 23 | 120 | no | 0 | no | `Doc-Start`, `algorithm.1` |
| stat_ML_2607.28567 | 44 | yes | no | 41 | 226 | no | 0 | no | `ALC@unique.1`, `ALC@unique.10` |
| un_report_un_sdg2023 | 80 | yes | no | 0 | 0 | no | 0 | yes |  |
| un_report_un_sdg2024 | 51 | yes | no | 22 | 22 | yes | 51 | yes | `p11`, `p13` |

## B. Re-saves: what each did to 8 files

Each file through: a qpdf plain rewrite (`qpdf in out`), `--linearize`, `--object-streams=generate`, `--object-streams=disable`, `mutool clean -gggg -z` (garbage-collect and recompress: the nearest thing here to an "optimise"), a PDFKit re-save, and a qpdf page insertion (page 1 copied in front, `--pages . 1,1-z`). "Content-stream digests" is SHA-256 of each page's decoded `/Contents` streams; the insertion's are compared with the original's from its second page on. "`recto text`" compares the SHA-256 and the character count of `recto text` for pages 1 and 2 (pages 2 and 3 after the insertion). Not tried: an incremental save by another program, an OCR pass (no OCR tool on this Mac) and Ghostscript's `pdfwrite` (not installed): these are listed as not measured in the report.

| File | Pages | Transform | ID[0] kept | ID[1] kept | Page object numbers kept | Content-stream digests kept | Page order kept | `recto text` p1, p2 identical | Named dests | Outline top level | Structure tree |
|---|---:|---|---|---|---:|---:|---|---|---:|---:|---|
| cs_CL_2607.28528 | 13 | original | | | | | | | 0 | 0 | yes |
| | | qpdf_plain | yes | no | 0/13 | 13/13 | yes | yes | 0 | 0 | yes |
| | | qpdf_linear | yes | no | 0/13 | 13/13 | yes | yes | 0 | 0 | yes |
| | | qpdf_objstm_on | yes | no | 1/13 | 13/13 | yes | yes | 0 | 0 | yes |
| | | qpdf_objstm_off | yes | no | 0/13 | 13/13 | yes | yes | 0 | 0 | yes |
| | | mutool_gc | yes | yes | 1/13 | 13/13 | yes | yes | 0 | 0 | yes |
| | | pdfkit_resave | no | no | 0/13 | 13/13 | yes | yes | 0 | 0 | yes |
| | | qpdf_dup_page1_front | yes | no | 0/13 | 13/13 | yes | yes | 0 | 0 | yes |
| standards_ietf_rfc9110 | 194 | original | | | | | | | 5345 | 2 | no |
| | | qpdf_plain | yes | no | 0/194 | 194/194 | yes | yes | 5345 | 2 | no |
| | | qpdf_linear | yes | no | 0/194 | 194/194 | yes | yes | 5345 | 2 | no |
| | | qpdf_objstm_on | yes | no | 0/194 | 194/194 | yes | yes | 5345 | 2 | no |
| | | qpdf_objstm_off | yes | no | 0/194 | 194/194 | yes | yes | 5345 | 2 | no |
| | | mutool_gc | yes | yes | 1/194 | 194/194 | yes | yes | 5345 | 2 | no |
| | | pdfkit_resave | no | no | 1/194 | 194/194 | yes | yes | 5345 | 2 | no |
| | | qpdf_dup_page1_front | yes | no | 0/194 | 194/194 | yes | yes | 5345 | 2 | no |
| forms_irs_f1040 | 2 | original | | | | | | | 0 | 0 | yes |
| | | qpdf_plain | yes | no | 0/2 | 2/2 | yes | yes | 0 | 0 | yes |
| | | qpdf_linear | yes | no | 1/2 | 2/2 | yes | yes | 0 | 0 | yes |
| | | qpdf_objstm_on | yes | no | 0/2 | 2/2 | yes | yes | 0 | 0 | yes |
| | | qpdf_objstm_off | yes | no | 0/2 | 2/2 | yes | yes | 0 | 0 | yes |
| | | mutool_gc | yes | yes | 1/2 | 2/2 | yes | yes | 0 | 0 | yes |
| | | pdfkit_resave | no | no | 0/2 | 2/2 | yes | yes | 0 | 0 | yes |
| | | qpdf_dup_page1_front | yes | no | 0/2 | 2/2 | yes | yes | 0 | 0 | yes |
| policy_worldbank_wbD34442277 | 20 | original | | | | | | | 0 | 0 | yes |
| | | qpdf_plain | yes | no | 1/20 | 20/20 | yes | yes | 0 | 0 | yes |
| | | qpdf_linear | yes | no | 19/20 | 20/20 | yes | yes | 0 | 0 | yes |
| | | qpdf_objstm_on | yes | no | 1/20 | 20/20 | yes | yes | 0 | 0 | yes |
| | | qpdf_objstm_off | yes | no | 1/20 | 20/20 | yes | yes | 0 | 0 | yes |
| | | mutool_gc | yes | yes | 3/20 | 20/20 | yes | yes | 0 | 0 | yes |
| | | pdfkit_resave | no | no | 0/20 | 20/20 | yes | yes | 0 | 0 | yes |
| | | qpdf_dup_page1_front | yes | no | 1/20 | 20/20 | yes | yes | 0 | 0 | yes |
| gov_nasa_ntrs20230009525 | 85 | original | | | | | | | 0 | 0 | yes |
| | | qpdf_plain | yes | no | 0/85 | 85/85 | yes | yes | 0 | 0 | yes |
| | | qpdf_linear | yes | no | 0/85 | 85/85 | yes | yes | 0 | 0 | yes |
| | | qpdf_objstm_on | yes | no | 1/85 | 85/85 | yes | yes | 0 | 0 | yes |
| | | qpdf_objstm_off | yes | no | 0/85 | 85/85 | yes | yes | 0 | 0 | yes |
| | | mutool_gc | yes | yes | 84/85 | 85/85 | yes | yes | 0 | 0 | yes |
| | | pdfkit_resave | no | no | 0/85 | 85/85 | yes | yes | 0 | 0 | yes |
| | | qpdf_dup_page1_front | yes | no | 0/85 | 85/85 | yes | yes | 0 | 0 | yes |
| eu_law_eu_dsa | 102 | original | | | | | | | 83 | 1 | yes |
| | | qpdf_plain | yes | no | 0/102 | 102/102 | yes | yes | 83 | 1 | yes |
| | | qpdf_linear | yes | no | 2/102 | 102/102 | yes | yes | 83 | 1 | yes |
| | | qpdf_objstm_on | yes | no | 1/102 | 102/102 | yes | yes | 83 | 1 | yes |
| | | qpdf_objstm_off | yes | no | 0/102 | 102/102 | yes | yes | 83 | 1 | yes |
| | | mutool_gc | yes | yes | 2/102 | 102/102 | yes | yes | 83 | 1 | yes |
| | | pdfkit_resave | no | no | 0/102 | 102/102 | yes | yes | 83 | 1 | yes |
| | | qpdf_dup_page1_front | yes | no | 0/102 | 102/102 | yes | yes | 83 | 1 | yes |
| gov_usgs_usgs70277796 | 2 | original | | | | | | | 13 | 10 | yes |
| | | qpdf_plain | yes | no | 0/2 | 2/2 | yes | yes | 13 | 10 | yes |
| | | qpdf_linear | yes | no | 1/2 | 2/2 | yes | yes | 13 | 10 | yes |
| | | qpdf_objstm_on | yes | no | 0/2 | 2/2 | yes | yes | 13 | 10 | yes |
| | | qpdf_objstm_off | yes | no | 0/2 | 2/2 | yes | yes | 13 | 10 | yes |
| | | mutool_gc | yes | yes | 1/2 | 2/2 | yes | yes | 13 | 10 | yes |
| | | pdfkit_resave | no | no | 0/2 | 2/2 | yes | yes | 13 | 10 | yes |
| | | qpdf_dup_page1_front | yes | no | 0/2 | 2/2 | yes | yes | 13 | 10 | yes |
| math_PR_2607.28413 | 24 | original | | | | | | | 165 | 5 | no |
| | | qpdf_plain | yes | no | 24/24 | 24/24 | yes | yes | 165 | 5 | no |
| | | qpdf_linear | yes | no | 0/24 | 24/24 | yes | yes | 165 | 5 | no |
| | | qpdf_objstm_on | yes | no | 0/24 | 24/24 | yes | yes | 165 | 5 | no |
| | | qpdf_objstm_off | yes | no | 0/24 | 24/24 | yes | yes | 165 | 5 | no |
| | | mutool_gc | yes | yes | 0/24 | 24/24 | yes | yes | 165 | 5 | no |
| | | pdfkit_resave | no | no | 0/24 | 24/24 | yes | yes | 165 | 5 | no |
| | | qpdf_dup_page1_front | yes | no | 18/24 | 24/24 | yes | yes | 165 | 5 | no |

## C. The full page digest under edits (10 files, 898 pages)

The page digest of the report's §5.2 (content streams decoded, length-prefixed; each XObject of the page's resources decoded with the general filters, or its encoded bytes for an image codec), as measured here with smaller caps than §5.2 sets: each XObject's first 1 MiB, no nested form resources, no cap on content; on cs_CL_2607.28528, standards_ietf_rfc9110, forms_irs_f1040, policy_worldbank_wbD34442277, gov_nasa_ntrs20230009525, eu_law_eu_dsa, gov_usgs_usgs70277796, math_PR_2607.28413, gov_nasa_ntrs20240002628 and astro-ph_GA_2607.28564. Pages compared are the pages both files have, aligned (the edited page left out where the edit is to page 1's content).

| Transform | Files | Pages compared | Digest unchanged |
|---|---:|---:|---:|
| qpdf plain rewrite | 8 | 442 | 442 |
| qpdf linearise | 8 | 442 | 442 |
| qpdf object streams on | 8 | 442 | 442 |
| qpdf object streams off | 8 | 442 | 442 |
| mutool clean -gggg -z | 8 | 442 | 442 |
| qpdf page 1 copied in front | 8 | 442 | 442 |
| PDFKit re-save | 18 | 1,340 | 1,340 |
| blank cover page inserted | 10 | 898 | 898 |
| page 1 deleted | 10 | 888 | 888 |
| last page deleted | 10 | 888 | 888 |
| page 1 drawn on (a stroke appended to its content) | 10 | 888 | 888 |
| highlight annotation added to page 1 | 10 | 898 | 898 |
| page 1 rotated 90° (`qpdf --rotate`, which sets `/Rotate`) | 10 | 898 | 898 |

A first version of the digest hashed XObjects' encoded bytes, and one page of gov_usgs_usgs70277796 changed under the rotation and the highlight: qpdf had rewritten an image whose `/Filter` was `[/FlateDecode]`, an array of one, re-encoding it (1,936 bytes to 1,614) with the same decoded pixels. Hence the general filters are decoded.

**Pages sharing a digest with another page of their own file:** with content streams alone, 46 of 5,872 (0.8%) in 7 files; with the XObjects too, 24 (0.4%) in 6 files: gov_nasa_ntrs20240002628, gov_nasa_ntrs20210011071, gov_nasa_ntrs20180005693, gov_nasa_ntrs20220005969, gov_usgs_usgs70275236, gov_usgs_usgs70277269 (each pair or group genuinely identical pages, by their digests: blank and separator pages).

## D. The document tag: collisions and robustness

Tags compared: `i`, SHA-256 of `/ID[0]`; `c1`, of page 1's digest; `c3`, of the first three pages' digests; `min8`, the smallest of the first eight pages' digests. All cut to 40 bits for the count.

**Collisions across the 141 files.** `i`: 140 distinct; cs_DS_2607.28574 and cs_DS_2607.28575, two different papers, share `/ID[0]`. `c1`, `c3`, `min8`: 138 distinct each; the groups are policy_worldbank_wbD34466295 with wbD34466311, and wbD34466334 with wbD34466335 and wbD34466337. Each group is one document published twice: same size, 184 bytes different (dates and `/ID`), every page's digest the same. So a content tag never put two different contents together here, and puts copies of one together, which is what "what it is, not where it lies" asks.

**Kept under an edit, of 10 files.**

| Transform | `i` | `c1` | `c3` | `min8` |
|---|---:|---:|---:|---:|
| page 1 rotated | 10 | 10 | 10 | 10 |
| highlight added | 10 | 10 | 10 | 10 |
| last page deleted | 10 | 10 | 8 | 8 |
| PDFKit re-save | 0 | 10 | 10 | 10 |
| blank cover inserted | 10 | 0 | 0 | 9 |
| page 1 deleted | 10 | 0 | 0 | 7 |
| page 1 drawn on | 10 | 0 | 0 | 8 |

`c3` and `min8` lose the 2-page files when their last page goes. Every transform keeps `i` or `c1`, which is why the report recommends carrying both.

**Cost.** Digesting the first eight pages took a median of 1.5 ms per file in Python (pikepdf), at most 0.9 s, on the 346 MiB one-page USGS map, whose page is the whole file: the cap on bytes read is what bounds it.

## E. Page text, for sizing an index

PyMuPDF's `get_text` over all 5,872 pages (an estimate of PDFium's: RectoPDF's normalised text differs in detail): 16.0 MiB of text, 2,849 bytes a page on average (median 2,717, 95th percentile 5,743, most 14,397); 2.66 million words, 452 a page; 72,622 distinct lowercased words.
