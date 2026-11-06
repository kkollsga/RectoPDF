// RectoPDF concept renders. Usage: concept.html?theme=light|dark&screen=reader|find|ask|sign|states
const qs = new URLSearchParams(location.search);
const theme = qs.get("theme") || "light";
const screen = qs.get("screen") || "reader";
document.documentElement.dataset.theme = theme;

const svg = (w, h, inner, extra = "") => `<svg width="${w}" height="${h}" viewBox="0 0 ${w} ${h}" fill="none" stroke="currentColor" stroke-width="1.45" stroke-linecap="round" stroke-linejoin="round" ${extra}>${inner}</svg>`;
const I = {
  search: svg(16, 16, '<circle cx="7" cy="7" r="4.6"/><path d="m10.6 10.6 3.4 3.4"/>'),
  ask: svg(16, 16, '<path d="M2.5 4.4c0-1 .8-1.9 1.9-1.9h7.2c1 0 1.9.9 1.9 1.9v5c0 1-.9 1.9-1.9 1.9H7.2L4.3 13.6v-2.3c-1 0-1.8-.8-1.8-1.9z"/><path d="m5.6 5.6 1.7 1.3-1.7 1.3M8.7 8.3h1.8"/>'),
  sign: svg(16, 16, '<path d="M10.8 2.6 13.4 5.2 6.3 12.3 2.8 13.2 3.7 9.7z"/><path d="M9.3 4.1 11.9 6.7"/>'),
  night: svg(16, 16, '<rect x="3.2" y="1.9" width="9.6" height="12.2" rx="1.4"/><path d="M8 4.6a3.4 3.4 0 0 1 0 6.8z" fill="currentColor" stroke="none"/>'),
  more: svg(16, 16, '<circle cx="3.4" cy="8" r=".9" fill="currentColor"/><circle cx="8" cy="8" r=".9" fill="currentColor"/><circle cx="12.6" cy="8" r=".9" fill="currentColor"/>'),
  plus: svg(16, 16, '<path d="M8 3v10M3 8h10"/>'),
  side: svg(16, 16, '<rect x="2" y="3" width="12" height="10" rx="1.8"/><path d="M6.3 3v10"/>'),
  chev: svg(10, 10, '<path d="m3.5 1.8 3.4 3.2-3.4 3.2"/>'),
  gear: svg(15, 15, '<circle cx="7.5" cy="7.5" r="2.1"/><path d="M7.5 1.6v1.7M7.5 11.7v1.7M1.6 7.5h1.7M11.7 7.5h1.7M3.3 3.3l1.2 1.2M10.5 10.5l1.2 1.2M3.3 11.7l1.2-1.2M10.5 4.5l1.2-1.2"/>'),
  folder: svg(15, 15, '<path d="M1.9 4.3c0-.7.6-1.3 1.3-1.3h2.6l1.4 1.5h4.6c.7 0 1.3.6 1.3 1.3v5.3c0 .7-.6 1.3-1.3 1.3H3.2c-.7 0-1.3-.6-1.3-1.3z"/>'),
  minus: svg(14, 14, '<path d="M3 7h8"/>'),
  plus14: svg(14, 14, '<path d="M7 3v8M3 7h8"/>'),
  fit: svg(14, 14, '<path d="M2 5V2.6c0-.3.3-.6.6-.6H5M9 2h2.4c.3 0 .6.3.6.6V5M12 9v2.4c0 .3-.3.6-.6.6H9M5 12H2.6c-.3 0-.6-.3-.6-.6V9"/>'),
  x: svg(14, 14, '<path d="m3.5 3.5 7 7M10.5 3.5l-7 7"/>'),
  up: svg(14, 14, '<path d="m3.5 8.8 3.5-3.6 3.5 3.6"/>'),
  down: svg(14, 14, '<path d="m3.5 5.2 3.5 3.6 3.5-3.6"/>'),
  send: svg(14, 14, '<path d="M7 11.5v-9M3.4 6 7 2.5 10.6 6"/>'),
  seal: svg(14, 14, '<path d="M7 1.3 8.5 2.5l1.9-.1.6 1.8 1.6 1.1-.7 1.8.7 1.7-1.6 1.1-.6 1.9-1.9-.1L7 12.7l-1.5-1.2-1.9.1-.6-1.9-1.6-1.1.7-1.7-.7-1.8L3 4.2l.6-1.8 1.9.1z"/><path d="m5 7 1.5 1.5L9.2 5.7"/>'),
  sigmark: svg(14, 14, '<path d="M7 1.3 8.5 2.5l1.9-.1.6 1.8 1.6 1.1-.7 1.8.7 1.7-1.6 1.1-.6 1.9-1.9-.1L7 12.7l-1.5-1.2-1.9.1-.6-1.9-1.6-1.1.7-1.7-.7-1.8L3 4.2l.6-1.8 1.9.1z"/>'),
  key: svg(15, 15, '<circle cx="5" cy="10" r="2.8"/><path d="m7 8 5.2-5.2M10 5l1.8 1.8"/>'),
  lock: svg(14, 14, '<rect x="3" y="6.2" width="8" height="5.8" rx="1.3"/><path d="M4.8 6.2V4.6a2.2 2.2 0 0 1 4.4 0v1.6"/>'),
  page: svg(14, 14, '<path d="M3.5 2.8c0-.5.4-.9.9-.9h3.7l2.4 2.4v6.9c0 .5-.4.9-.9.9H4.4a.9.9 0 0 1-.9-.9z"/><path d="M8 2v2.4h2.4"/>'),
  claude: svg(12, 12, '<circle cx="6" cy="6" r="1.5" fill="currentColor" stroke="none"/>'),
};

const thumb = (kind = "text") => {
  const body = { text: "<b></b><b></b><b class=s></b><b></b><b></b><b class=s></b>", fig: "<b></b><u></u><b></b><b class=s></b>", form: "<b class=s></b><b></b><b></b><b class=s></b><b></b>" }[kind];
  return `<span class="thumb"><b class="h"></b>${body}</span>`;
};

// TOC rail doubles as reading progress: it fills from the top down to where you are.
const SURVEY_TOC = [
  ["1  Summary", 3], ["2  Survey method", 7], ["3  Sediment and scour", 24, "up"],
  ["3.1  Seabed composition", 25, "sub"], ["3.2  Scour at the ferry quay", 32, "sub here"], ["3.3  Breakwater toe", 38, "sub"],
  ["4  Dredging priorities", 52], ["5  Cost estimates", 67], ["A  Transect data", 79], ["B  Instruments", 91],
];
const LEASE_TOC = [["1  Parties", 1], ["2  Premises", 1], ["3  Rent", 2], ["4  Term and notice", 3], ["5  Maintenance", 4], ["6  Signatures", 6, "here"]];
const TABS = [
  { id: "survey", name: "Nordvik Seabed Survey 2025", kind: "fig", at: 32, toc: SURVEY_TOC },
  { id: "lease", name: "Quay Shed 14 Lease", kind: "form", at: 6, toc: LEASE_TOC, seal: true },
  { id: "tide", name: "Tide Tables Q1 2026", kind: "text", at: 9 },
  { id: "notes", name: "PDF 2.0 Reading Notes", kind: "text", at: 41 },
  { id: "tender", name: "Dredging Tender Form", kind: "form", at: 2 },
  { id: "moor", name: "Mooring Standards Vol 2", kind: "fig", at: 118 },
  { id: "inv", name: "Invoice 2026-0412", kind: "form", at: 1 },
];

function tocHtml(items, showPages = false) {
  const i = items.findIndex((x) => (x[2] || "").includes("here"));
  const fill = i * 28 + 14 - 6;
  return `<ul class="toc ${showPages ? "showp" : ""}"><span class="fillrail" style="height:${fill}px"></span><span class="knob" style="top:${i * 28 + 10.2}px"></span>
    ${items.map(([t, p, c = ""]) => `<li class="${c}"><span class="t">${t}</span><span class="p">${p}</span></li>`).join("")}</ul>`;
}

// At rest: names and contents only. Page numbers and the chevron appear on hover.
function sidebar({ active = "survey", open = true, tabs = TABS, tap = false, hover = false, on = "", pg = [32, 96], body = null, chrome = true, menu = "" } = {}) {
  const rows = tabs.map((t) => {
    const isOn = t.id === active;
    return `<div class="tab ${isOn ? "active" : ""}"><span class="name">${t.name}</span>
      ${isOn && (hover || !open) ? `<span class="chev ${open ? "open" : ""}">${I.chev}</span>` : ""}</div>
      ${isOn && body ? body : ""}${isOn && open && !body && t.toc ? tocHtml(t.toc, hover) : ""}`;
  }).join("");
  return `<aside class="sidebar">
    <div class="side-top"><div class="lights"><i></i><i></i><i></i></div><span class="ibtn ${on.includes("search") ? "on" : ""}">${I.search}</span><span class="ibtn">${I.plus}</span></div>
    <div class="tabs" style="position:relative">${rows}${tap ? '<span class="tap"></span>' : ""}</div>
    <div class="side-foot">${chrome ? `<span class="place"><b>${pg[0]}</b> of ${pg[1]}</span>` : "<span></span>"}<span class="ibtn ${on.includes("more") ? "on" : ""}">${I.more}</span></div>
    ${menu}
  </aside>`;
}

const AGENT = (on = false) => `<span class="ibtn agentbtn ${on ? "on" : ""}">${I.ask}</span>`;
const pill = (page, total) => `<div class="pill"><span class="sec"><span class="num">${page}</span> of ${total}</span></div>`;
const MENU = () => `<div class="menu">
  <div class="mi">Find<kbd>⌘F</kbd></div>
  <div class="mi zoomrow">Zoom<span class="zoom"><span class="zb">${I.minus}</span>125%<span class="zb">${I.plus14}</span><span class="zb">${I.fit}</span></span></div>
  ${theme === "dark" ? `<div class="mi paperrow">Paper<span class="seg mini"><span class="on">Dark</span><span>White</span></span></div>` : ""}
  <div class="msep"></div>
  <div class="mi">Sign…</div><div class="mi">Print…<kbd>⌘P</kbd></div><div class="mi">Hide sidebar<kbd>⌃⌘S</kbd></div>
  <div class="msep"></div>
  <div class="mi">Settings…<kbd>⌘,</kbd></div></div>`;

// A bathymetry figure with its own colours: it keeps them in PDF dark mode, like photos do.
const FIGURE = `<svg viewBox="0 0 576 214" xmlns="http://www.w3.org/2000/svg" font-family="-apple-system, Helvetica Neue, sans-serif">
  <rect width="576" height="214" fill="#e4f1f6"/>
  <path d="M0 214V120C70 96 118 150 190 128s96-66 176-62 130 34 210 12v136z" fill="#c6e2ee"/>
  <path d="M60 214c10-48 70-52 128-62s92-52 170-50 126 36 218 24v88z" fill="#9fcde2"/>
  <path d="M150 214c14-34 58-38 100-44s72-40 124-40 110 30 202 26v58z" fill="#6fafd0"/>
  <path d="M238 214c10-20 34-26 62-30s50-30 88-28 52 32 72 58z" fill="#3f8cb6"/>
  <path d="M312 214c6-10 18-16 34-18s30-14 46-8 18 18 24 26z" fill="#1f628c"/>
  <rect x="0" y="0" width="576" height="26" fill="#d5cdbb"/><rect x="0" y="26" width="576" height="4" fill="#7c7464"/>
  <text x="14" y="18" font-size="10.5" fill="#4a4436">Ferry quay, northern berth</text>
  <g stroke="#17354a" stroke-width=".8" stroke-dasharray="3 3">
    <path d="M150 50V214M222 50V214M294 50V214M366 50V214M438 50V214"/>
  </g>
  <g font-size="9.5" fill="#17354a" font-weight="600" text-anchor="middle">
    <text x="150" y="44">T-12</text><text x="222" y="44">T-13</text><text x="294" y="44">T-14</text><text x="366" y="44">T-15</text><text x="438" y="44">T-16</text>
  </g>
  <circle cx="366" cy="200" r="4" fill="#fff" stroke="#17354a"/>
  <g font-size="9" fill="#10293a"><rect x="478" y="42" width="86" height="60" fill="#ffffffd9" rx="2"/>
    <text x="486" y="55" font-weight="600">Depth below</text><text x="486" y="66" font-weight="600">design level</text>
    <rect x="486" y="72" width="66" height="7" fill="url(#g)"/><text x="486" y="93">0 m</text><text x="534" y="93">2.5 m</text></g>
  <defs><linearGradient id="g"><stop stop-color="#c6e2ee"/><stop offset="1" stop-color="#1f628c"/></linearGradient></defs>
</svg>`;

const h = (w, now = false) => `<mark class="hit ${now ? "now" : ""}">${w}</mark>`;
const SURVEY_PAGE = `<article class="page">
  <div class="head"><span>Nordvik Harbour Authority · Seabed Survey 2025</span><span>Sediment and scour</span></div>
  <h2><span>3.2</span>${h("Scour")} at the ferry quay</h2>
  <p>The ferry quay sits on the outer edge of the harbour basin, where propeller wash from the two daily crossings meets the ebb current. Repeated multibeam passes between March and September 2025 show that the ${h("scour", true)} hole along the berth face has continued to deepen, although more slowly than the 2023 survey predicted.</p>
  <p><mark class="cite">The deepest point, at transect T-14, now lies 2.4 m below the design seabed level, compared with 1.9 m in 2023.</mark> ${h("Scour")} reaches within 6 m of the sheet-pile toe on the northern berth, while the southern berth remains stable.</p>
  <figure class="fig">${FIGURE}
    <figcaption class="cap"><b>Figure 3.4.</b> Bathymetry along the northern berth, September 2025, with survey transects T-12 to T-16.</figcaption></figure>
  <figcaption class="cap" style="margin-top:14px"><b>Table 3.2.</b> ${h("Scour")} depth by transect, metres below design seabed level.</figcaption>
  <table>
    <tr><th>Transect</th><th>2023</th><th>2025</th><th>Change</th><th>Distance to piles</th></tr>
    <tr><td>T-12</td><td>1.1</td><td>1.3</td><td>+0.2</td><td>14 m</td></tr>
    <tr><td>T-13</td><td>1.6</td><td>1.8</td><td>+0.2</td><td>9 m</td></tr>
    <tr class="hot"><td>T-14</td><td>1.9</td><td>2.4</td><td>+0.5</td><td>6 m</td></tr>
    <tr><td>T-15</td><td>1.4</td><td>1.7</td><td>+0.3</td><td>8 m</td></tr>
    <tr><td>T-16</td><td>0.8</td><td>0.9</td><td>+0.1</td><td>17 m</td></tr>
  </table>
  <p>The rate of deepening has slowed since the crossing timetable changed in May 2024, when the morning departure moved away from the lowest ebb. Even so, the margin at T-14 falls below the 8 m clearance set in the quay's design basis, which makes this the first location to address in the dredging and armouring programme in Chapter 4.</p>
  <div class="pageno">32</div>
</article>`;

const ROUGH_SIG = `<svg viewBox="0 0 150 50" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round"><path d="M6 36c6-14 14-28 19-26s-8 34-3 32 12-19 17-18-3 16 3 15 9-13 14-12-1 12 5 11 7-9 12-8 2 8 7 7 8-6 13-5 3 5 9 3 10-5 20-6"/></svg>`;
const OWN_SIG = `<svg viewBox="0 0 150 50" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"><path d="M10 34C18 22 22 8 26 9s-5 28-1 28 9-14 13-14-2 12 3 11 8-10 11-9-2 9 3 8 7-7 10-6 0 6 5 5 15-5 30-9c7-2 14-3 22-2"/></svg>`;

const LEASE_PAGE = `<article class="page contract true-page">
  <div class="head"><span>Lease agreement · Quay Shed 14</span><span>Page 6 of 6</span></div>
  <h2><span>6</span>Signatures</h2>
  <p>Each party signs below to confirm that it has read this lease and agrees to its terms, including the rent in clause 3, the term and notice period in clause 4, and the maintenance duties in clause 5. The lease takes effect on the date of the last signature.</p>
  <p>This agreement is made in electronic form. Each party receives a copy of the signed file, and the signed file is the original of the agreement. A party that wishes to confirm the signatures may do so in any reader that supports certificate-based PDF signatures.</p>
  <p>Any change to this lease after signing must be made in writing and signed by both parties. A later change does not affect the validity of the signatures on this version.</p>
  <div class="sigs">
    <div class="sigblock"><div class="who">For the landlord<br>Nordvik Harbour Authority</div>
      <div class="sigbox done">${ROUGH_SIG}<div class="stamp">Digitally signed by Tor Haugen<br>2 November 2026, 14:06</div></div></div>
    <div class="sigblock"><div class="who">For the tenant<br>Solheim Marine Services</div>
      <div class="sigbox todo">Sign here</div></div>
  </div>
  <div class="sigs" style="margin-top:14px">
    <div></div>
    <div><div class="fieldrow"><span style="width:38px">Name</span><span class="box">Ingrid Solheim</span></div>
    <div class="fieldrow"><span style="width:38px">Title</span><span class="box">Managing Director</span></div></div>
  </div>
  <div class="pageno">6</div>
</article>`;

const stub = `<div class="page stub"></div>`;

const column = (page, top, width) => {
  const scale = width / 720;
  return `<div class="sheet-column" style="top:${top}px;transform:translateX(-50%) scale(${scale})">${page}</div>`;
};

function surface({ page = SURVEY_PAGE, width = 760, cls = "", top = -14, withStub = true, extra = "" }) {
  return `<section class="surface ${cls}">${extra}${column((withStub ? stub : "") + page, top, width)}</section>`;
}

const ASK = `<aside class="panel">
  <div class="p-head"><h3>Ask</h3><span class="chip" style="margin-left:2px">Claude Code · Sonnet</span><span style="flex:1"></span><span class="ibtn">${I.plus}</span>${AGENT(true)}</div>
  <div class="p-body">
    <div class="q-bubble">How deep is the scour at the ferry quay, and has it changed since 2023?</div>
    <div class="trace">
      <div>${I.search}<span>Searched <code>scour</code> 23 hits on 6 pages</span></div>
      <div>${I.search}<span>Searched <code>armou?r|rock fill</code> 4 hits on 2 pages</span></div>
      <div>${I.page}<span>Read pages 32, 33 and 54</span></div>
    </div>
    <div class="answer">
      <p>The deepest scour is <b>2.4 m below the design seabed level</b>, at transect T-14. <span class="cref">p. 32</span></p>
      <p>That is 0.5 m deeper than the 1.9 m measured in 2023, the largest increase of the five transects. <span class="cref">p. 32</span> The rate has slowed since the crossing timetable changed in May 2024. <span class="cref">p. 32</span></p>
      <p>The report ranks T-14 first for rock armouring, because it now sits 6 m from the sheet piles, against a design clearance of 8 m. <span class="cref">p. 54</span></p>
    </div>
    <div class="tally">Read 3 of 96 pages, about 1,900 tokens of document text</div>
  </div>
  <div class="composer"><div class="ph">Ask about this document</div><div class="row"><span class="chip">${I.lock}Read and search only</span><span class="send">${I.send}</span></div></div>
</aside>`;

const SIGN = `<aside class="panel">
  <div class="p-head"><h3>Sign</h3><span style="flex:1"></span><span class="ibtn">${I.x}</span></div>
  <div class="p-body">
    <div class="seg"><span class="on">Digital signature</span><span>Image only</span></div>
    <div class="hint">Links the file to your certificate and shows any later change.</div>
    <div class="lab">Certificate</div>
    <div class="field cert"><span class="ic">${I.key}</span><div><div class="who">Ingrid Solheim</div><div class="sub">In Keychain. Self-signed, so others see a warning</div></div><span class="chev">${I.chev}</span></div>
    <div class="lab">How it looks on the page</div>
    <div class="field preview">${OWN_SIG}<div class="small">Digitally signed by<br>Ingrid Solheim<br>6 November 2026</div></div>
    <div class="lab">Reason</div>
    <div class="field input">I agree to this lease</div>
    <div class="toggle-row"><div><div class="tt">Add a trusted timestamp</div><div class="ts">Proves when you signed. Sends timestamp.example only a fingerprint of the file, never its content.</div></div><span class="switch"></span></div>
    <div class="note">${I.sigmark}<span>Saved as a new revision. The landlord's signature still covers the version they signed.</span></div>
  </div>
  <div class="actions"><span class="btn ghost">Cancel</span><span class="btn primary">Sign</span></div>
</aside>`;

const hits = [["31", "<em>Scour</em> and sediment transport at the quay", 2], ["32", "<em>Scour</em> at the ferry quay", 4, "here"], ["33", "the <em>scour</em> hole is expected to stabilise", 4], ["38", "toe <em>scour</em> along the breakwater", 7], ["54", "<em>scour</em> protection at T-14 comes first", 4], ["79", "transect data, <em>scour</em> depth by year", 2]];
const FIND = `<div class="findbox"><div class="fieldline">${I.search}<span class="q">scour<span class="caret"></span></span><span class="count">4 of 23</span><span class="zb">${I.up}</span><span class="zb">${I.down}</span></div>
  <ul class="hitlist">${hits.map(([p, t, n, c = ""]) => `<li class="${c}"><span class="hp">${p}</span><span class="ht">${t}</span></li>`).join("")}</ul></div>`;
const STATUS = `<div class="sigline">1 signature<span>Details</span></div>`;

const MARK = (s) => `<svg width="${s}" height="${s}" viewBox="0 0 64 64" fill="none"><path d="M31 15 11 11c-1.2-.2-2 .6-2 1.7v35c0 1 .6 1.7 1.6 1.9L31 54.5z" fill="var(--fill-strong)"/><path d="M33 15l20-4c1.2-.2 2 .6 2 1.7v35c0 1-.6 1.7-1.6 1.9L33 54.5z" fill="var(--accent)"/><path d="M39 23.5l10-2M39 31l10-2M39 38.500l6-1.200" stroke="var(--accent-ink)" stroke-width="2.200" stroke-linecap="round" opacity=".85"/></svg>`;
const RECENT = [["Nordvik Seabed Survey 2025", "Page 32 of 96", "fig"], ["Quay Shed 14 Lease", "Signed, page 6 of 6", "form"], ["Tide Tables Q1 2026", "Page 9 of 44", "text"], ["PDF 2.0 Reading Notes", "Page 41 of 212", "text"]];
const HOME = `<section class="surface home"><div class="home-in">
  ${MARK(68)}<h1>RectoPDF</h1><p class="home-sub">Drop a PDF here, or</p><span class="btn primary">Open a PDF</span>
  <div class="recents">${RECENT.map(([n, m, k]) => `<div class="rec"><div class="rec-page ${k}"><i></i><b></b><b></b><u></u>${"<b></b>".repeat(k === "fig" ? 6 : 13)}</div><div class="rec-n">${n}</div><div class="rec-m">${m}</div></div>`).join("")}</div>
</div></section>`;

const Screens = {
  home: () => `<div class="window">${sidebar({ active: "", tabs: [], chrome: false })}<main class="main">${HOME}</main></div>`,
  reader: () => `<div class="window">${sidebar({})}<main class="main">${surface({ extra: AGENT() })}</main></div>`,
  paperwhite: () => `<div class="window">${sidebar({})}<main class="main">${surface({ cls: "true-page", extra: AGENT() })}</main></div>`,
  menu: () => `<div class="window">${sidebar({ on: "more", menu: MENU() })}<main class="main">${surface({ extra: AGENT() })}</main></div>`,
  find: () => `<div class="window">${sidebar({ on: "search", body: FIND })}<main class="main">${surface({ cls: "show-hits", extra: AGENT() })}</main></div>`,
  ask: () => `<div class="window">${sidebar({})}<main class="main">${surface({ cls: "show-cite", width: 700 })}${ASK}</main></div>`,
  sign: () => `<div class="window">${sidebar({ active: "lease", pg: [6, 6], body: STATUS + tocHtml(LEASE_TOC) })}<main class="main">${surface({ page: LEASE_PAGE, width: 700 })}${SIGN}</main></div>`,
  focus: () => `<div class="window focus"><main class="main">${surface({ width: 900, extra: AGENT() })}<div class="pill"><span class="sec"><span class="num">32</span> of 96</span></div></main></div>`,
  states: () => {
    const half = (t) => `<div class="half ${t === "dark" ? "d" : "l"}" data-theme="${t}">
      <div class="pair">
        <div class="frame" style="position:relative">${sidebar({ open: true, tap: true, hover: true })}</div>
        <div class="arrow">${svg(40, 16, '<path d="M2 8h35M31 3l6 5-6 5"/>')}Tap the<br>active PDF</div>
        <div class="frame">${sidebar({ open: false })}</div>
      </div>
      <div class="board-cap"><b>Tap the active PDF to fold its contents away.</b> The rail fills as you read, so it doubles as your place in the document. Hovering shows the chevron and page numbers, as on the left. At rest the sidebar is only names.</div>
    </div>`;
    return `<div class="board">${half("light")}${half("dark")}</div>`;
  },
};

document.getElementById("stage").innerHTML = (Screens[screen] || Screens.reader)();
