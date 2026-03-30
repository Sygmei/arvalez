import { a0 as head, e as escape_html, a1 as attr, a2 as ensure_array_like, a3 as attr_class } from "../../chunks/index.js";
function _page($$renderer, $$props) {
  $$renderer.component(($$renderer2) => {
    let graphSeries, issueGroups, activeGroup;
    let summaries = [];
    let selectedReportFile = null;
    let currentReport = null;
    let selectedIssueKey = null;
    let selectedIssueInstanceId = null;
    let issueSearch = "";
    let statusMessage = "Connecting to report directory…";
    let shouldAutoSelectIssueGroup = true;
    function formatDate(unixSeconds) {
      return new Date(unixSeconds * 1e3).toLocaleString();
    }
    function supportPercent(report) {
      if (!report || report.total_specs === 0) {
        return 0;
      }
      return report.passed_specs / report.total_specs * 100;
    }
    function buildGraphPoints(reports) {
      if (reports.length === 0) {
        return [];
      }
      const width = 960;
      const height = 280;
      const padLeft = 40;
      const padRight = 12;
      const padTop = 14;
      const padBottom = 26;
      const plotWidth = width - padLeft - padRight;
      const plotHeight = height - padTop - padBottom;
      return reports.map((report, index) => {
        const x = reports.length === 1 ? padLeft + plotWidth / 2 : padLeft + plotWidth * index / (reports.length - 1);
        const y = padTop + plotHeight - supportPercent(report) / 100 * plotHeight;
        return { report, x, y };
      });
    }
    function graphPath(points) {
      return points.map((point, index) => `${index === 0 ? "M" : "L"} ${point.x} ${point.y}`).join(" ");
    }
    function flattenedFailures(report) {
      {
        return [];
      }
    }
    function groupedFailures(report, searchTerm) {
      const normalized = searchTerm.trim().toLowerCase();
      const groups = /* @__PURE__ */ new Map();
      for (const item of flattenedFailures()) {
        const key = `${item.failure.kind}:${item.failure.feature}`;
        const haystack = [
          item.failure.kind,
          item.failure.feature,
          item.failure.pointer,
          item.failure.schema_path,
          item.failure.source_preview,
          item.spec,
          item.target,
          item.failure.message
        ].filter(Boolean).join("\n").toLowerCase();
        if (normalized && !haystack.includes(normalized)) {
          continue;
        }
        if (!groups.has(key)) {
          groups.set(key, {
            key,
            kind: item.failure.kind,
            feature: item.failure.feature,
            count: 0,
            items: []
          });
        }
        const group = groups.get(key);
        group.count += 1;
        group.items.push(item);
      }
      return [...groups.values()].sort((left, right) => right.count - left.count || left.kind.localeCompare(right.kind) || left.feature.localeCompare(right.feature));
    }
    function pickSelectedGroup(groups, issueKey) {
      return groups.find((group) => group.key === issueKey) ?? groups[0] ?? null;
    }
    function pickSelectedIssue(group, issueInstanceId) {
      if (!group) {
        return null;
      }
      if (!issueInstanceId) {
        return group.items[0] ?? null;
      }
      return group.items.find((item) => item.id === issueInstanceId) ?? group.items[0] ?? null;
    }
    graphSeries = buildGraphPoints(summaries);
    issueGroups = groupedFailures(currentReport, issueSearch);
    {
      const groups = issueGroups;
      if (!groups.length) {
        selectedIssueKey = null;
        selectedIssueInstanceId = null;
      } else if (shouldAutoSelectIssueGroup && (!selectedIssueKey || !groups.some((group) => group.key === selectedIssueKey))) {
        selectedIssueKey = groups[0].key;
        selectedIssueInstanceId = groups[0].items[0]?.id ?? null;
        shouldAutoSelectIssueGroup = false;
      } else {
        const group = groups.find((value) => value.key === selectedIssueKey);
        if (group && (!selectedIssueInstanceId || !group.items.some((item) => item.id === selectedIssueInstanceId))) {
          selectedIssueInstanceId = group.items[0]?.id ?? null;
        }
      }
    }
    activeGroup = pickSelectedGroup(issueGroups, selectedIssueKey);
    pickSelectedIssue(activeGroup, selectedIssueInstanceId);
    head("1uha8ag", $$renderer2, ($$renderer3) => {
      $$renderer3.title(($$renderer4) => {
        $$renderer4.push(`<title>Arvalez Corpus Dashboard</title>`);
      });
    });
    $$renderer2.push(`<div class="shell svelte-1uha8ag"><header class="hero panel svelte-1uha8ag"><div class="hero-copy svelte-1uha8ag"><div class="eyebrow svelte-1uha8ag">Arvalez Corpus Dashboard</div> <h1 class="svelte-1uha8ag">Live Report Explorer</h1> <p class="svelte-1uha8ag">Watching <code class="svelte-1uha8ag">${escape_html("REPORT_DIRECTORY")}</code> for new <code class="svelte-1uha8ag">apis-guru-*.json</code> reports.</p></div> `);
    {
      $$renderer2.push("<!--[-1-->");
    }
    $$renderer2.push(`<!--]--></header> <div class="status-row svelte-1uha8ag"><span>${escape_html(statusMessage)}</span> `);
    {
      $$renderer2.push("<!--[-1-->");
    }
    $$renderer2.push(`<!--]--></div> <section class="overview-layout svelte-1uha8ag"><section class="panel overview-panel svelte-1uha8ag"><div class="section-header svelte-1uha8ag"><div><div class="eyebrow svelte-1uha8ag">Progression</div> <h2 class="svelte-1uha8ag">Support Over Time</h2></div> <div class="subtle svelte-1uha8ag">`);
    {
      $$renderer2.push("<!--[-1-->");
    }
    $$renderer2.push(`<!--]--></div></div> `);
    if (summaries.length === 0) {
      $$renderer2.push("<!--[0-->");
      $$renderer2.push(`<p>No reports loaded yet.</p>`);
    } else {
      $$renderer2.push("<!--[-1-->");
      $$renderer2.push(`<svg class="trend-chart svelte-1uha8ag" viewBox="0 0 960 280" preserveAspectRatio="none" aria-label="Support trend"><line class="axis svelte-1uha8ag" x1="40" y1="14" x2="40" y2="254"></line><line class="axis svelte-1uha8ag" x1="40" y1="254" x2="948" y2="254"></line><text x="40" y="18" class="axis-label svelte-1uha8ag">100%</text><text x="40" y="272" class="axis-label svelte-1uha8ag">0%</text><path class="series svelte-1uha8ag"${attr("d", graphPath(graphSeries))}></path><!--[-->`);
      const each_array = ensure_array_like(graphSeries);
      for (let $$index = 0, $$length = each_array.length; $$index < $$length; $$index++) {
        let point = each_array[$$index];
        $$renderer2.push(`<circle${attr_class("point svelte-1uha8ag", void 0, { "selected": selectedReportFile === point.report.file })}${attr("cx", point.x)}${attr("cy", point.y)} r="5" role="button" tabindex="0"><title>${escape_html(formatDate(point.report.generated_at_unix_seconds))} · ${escape_html(supportPercent(point.report).toFixed(1))}%</title></circle>`);
      }
      $$renderer2.push(`<!--]--></svg>`);
    }
    $$renderer2.push(`<!--]--></section> <section class="panel overview-panel top-failures-panel svelte-1uha8ag"><div class="section-header svelte-1uha8ag"><div><div class="eyebrow svelte-1uha8ag">Summary</div> <h2 class="svelte-1uha8ag">Top Failure Groups</h2></div></div> `);
    {
      $$renderer2.push("<!--[-1-->");
      $$renderer2.push(`<p class="subtle svelte-1uha8ag">Waiting for a report selection.</p>`);
    }
    $$renderer2.push(`<!--]--></section></section> <section class="panel issues-panel svelte-1uha8ag"><div class="section-header svelte-1uha8ag"><div><div class="eyebrow svelte-1uha8ag">Failures</div> <h2 class="svelte-1uha8ag">Grouped Issues</h2></div> <input class="search svelte-1uha8ag" type="search"${attr("value", issueSearch)} placeholder="Filter kind, feature, pointer, spec…"/></div> `);
    {
      $$renderer2.push("<!--[0-->");
      $$renderer2.push(`<p>Select a report to inspect its issues.</p>`);
    }
    $$renderer2.push(`<!--]--></section> <section class="panel history-panel svelte-1uha8ag"><div class="section-header svelte-1uha8ag"><div><div class="eyebrow svelte-1uha8ag">History</div> <h2 class="svelte-1uha8ag">Loaded Reports</h2></div></div> <div class="history-list svelte-1uha8ag"><!--[-->`);
    const each_array_4 = ensure_array_like([...summaries].reverse());
    for (let $$index_4 = 0, $$length = each_array_4.length; $$index_4 < $$length; $$index_4++) {
      let report = each_array_4[$$index_4];
      $$renderer2.push(`<button type="button"${attr_class("history-item svelte-1uha8ag", void 0, { "selected": selectedReportFile === report.file })}><strong>${escape_html(formatDate(report.generated_at_unix_seconds))}</strong> <div class="history-meta svelte-1uha8ag"><span>${escape_html(supportPercent(report).toFixed(1))}%</span> <span>${escape_html(report.passed_specs)}/${escape_html(report.total_specs)} passed</span></div> <code class="svelte-1uha8ag">${escape_html(report.file)}</code></button>`);
    }
    $$renderer2.push(`<!--]--></div></section></div>`);
  });
}
export {
  _page as default
};
