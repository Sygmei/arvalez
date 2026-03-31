<script>
  import { onMount } from "svelte";

  let reportDirectory = "";
  let summaries = [];
  let selectedReportFile = null;
  let currentReport = null;
  let selectedIssueKey = null;
  let selectedIssueInstanceId = null;
  let issueSearch = "";
  let loadError = "";
  let statusMessage = "Connecting to report directory…";
  let shouldAutoSelectIssueGroup = true;
  let lastSelectedReportFile = null;

  function formatDate(unixSeconds) {
    return new Date(unixSeconds * 1000).toLocaleString();
  }

  function supportPercent(report) {
    if (!report || report.total_specs === 0) {
      return 0;
    }
    return (report.passed_specs / report.total_specs) * 100;
  }

  function totalWarnings(report) {
    if (!report) {
      return 0;
    }
    return (report.results ?? []).reduce(
      (sum, result) => sum + (result.warning_count ?? 0),
      0,
    );
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
      const x =
        reports.length === 1
          ? padLeft + plotWidth / 2
          : padLeft + (plotWidth * index) / (reports.length - 1);
      const y =
        padTop + plotHeight - (supportPercent(report) / 100) * plotHeight;
      return { report, x, y };
    });
  }

  function graphPath(points) {
    return points
      .map((point, index) => `${index === 0 ? "M" : "L"} ${point.x} ${point.y}`)
      .join(" ");
  }

  function flattenedFailures(report) {
    if (!report) {
      return [];
    }

    const failures = [];
    for (const result of report.results ?? []) {
      if (result.failure) {
        failures.push({
          id: `${result.spec}::spec`,
          spec: result.spec,
          target: null,
          generatedFiles: null,
          warningCount: result.warning_count ?? 0,
          failure: result.failure,
        });
      }

      for (const targetResult of result.targets ?? []) {
        if (!targetResult.failure) {
          continue;
        }
        failures.push({
          id: `${result.spec}::${targetResult.name}`,
          spec: result.spec,
          target: targetResult.name,
          generatedFiles: targetResult.generated_files ?? 0,
          warningCount: result.warning_count ?? 0,
          failure: targetResult.failure,
        });
      }
    }

    return failures;
  }

  function groupedFailures(report, searchTerm) {
    const normalized = searchTerm.trim().toLowerCase();
    const groups = new Map();

    for (const item of flattenedFailures(report)) {
      const key = `${item.failure.kind}:${item.failure.feature}`;
      const haystack = [
        item.failure.kind,
        item.failure.feature,
        item.failure.pointer,
        item.failure.schema_path,
        item.failure.source_preview,
        item.spec,
        item.target,
        item.failure.message,
      ]
        .filter(Boolean)
        .join("\n")
        .toLowerCase();

      if (normalized && !haystack.includes(normalized)) {
        continue;
      }

      if (!groups.has(key)) {
        groups.set(key, {
          key,
          kind: item.failure.kind,
          feature: item.failure.feature,
          count: 0,
          items: [],
        });
      }

      const group = groups.get(key);
      group.count += 1;
      group.items.push(item);
    }

    return [...groups.values()].sort(
      (left, right) =>
        right.count - left.count ||
        left.kind.localeCompare(right.kind) ||
        left.feature.localeCompare(right.feature),
    );
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
    return (
      group.items.find((item) => item.id === issueInstanceId) ??
      group.items[0] ??
      null
    );
  }

  async function refreshReports() {
    const response = await fetch("/api/reports");
    if (!response.ok) {
      throw new Error(
        `Failed to load reports: ${response.status} ${response.statusText}`,
      );
    }

    const payload = await response.json();
    reportDirectory = payload.reportDirectory ?? "";
    const nextSummaries = payload.reports ?? [];
    const previousLatest = summaries.at(-1)?.file ?? null;
    summaries = nextSummaries;

    const nextLatest = nextSummaries.at(-1)?.file ?? null;
    const shouldFollowLatest =
      !selectedReportFile ||
      selectedReportFile === previousLatest ||
      !nextSummaries.some((item) => item.file === selectedReportFile);

    if (shouldFollowLatest) {
      selectedReportFile = nextLatest;
      shouldAutoSelectIssueGroup = true;
    }

    statusMessage =
      nextSummaries.length === 0
        ? `Watching ${reportDirectory}. No reports found yet.`
        : `Watching ${reportDirectory}. Loaded ${nextSummaries.length} report${nextSummaries.length === 1 ? "" : "s"}.`;
  }

  async function refreshCurrentReport() {
    if (!selectedReportFile) {
      currentReport = null;
      return;
    }

    const response = await fetch(
      `/api/report?file=${encodeURIComponent(selectedReportFile)}`,
    );
    if (!response.ok) {
      throw new Error(
        `Failed to load report ${selectedReportFile}: ${response.status} ${response.statusText}`,
      );
    }
    currentReport = await response.json();
  }

  onMount(() => {
    let disposed = false;
    let eventSource;

    const boot = async () => {
      try {
        await refreshReports();
        await refreshCurrentReport();
        loadError = "";
      } catch (error) {
        loadError = error instanceof Error ? error.message : String(error);
      }
    };

    boot();

    eventSource = new EventSource("/api/reports/stream");
    eventSource.onmessage = async (event) => {
      if (disposed) {
        return;
      }

      try {
        const payload = JSON.parse(event.data);
        if (payload.type === "directory") {
          reportDirectory = payload.reportDirectory ?? reportDirectory;
          return;
        }
        if (payload.type !== "updated" && payload.type !== "ready") {
          return;
        }

        const previousSelected = selectedReportFile;
        await refreshReports();
        if (
          previousSelected !== selectedReportFile ||
          payload.type === "updated"
        ) {
          await refreshCurrentReport();
        }
        loadError = "";
      } catch (error) {
        loadError = error instanceof Error ? error.message : String(error);
      }
    };

    eventSource.onerror = () => {
      loadError =
        "Lost connection to the report watcher. Reload the page or restart the dev server.";
    };

    return () => {
      disposed = true;
      eventSource?.close();
    };
  });

  $: graphSeries = buildGraphPoints(summaries);
  $: issueGroups = groupedFailures(currentReport, issueSearch);
  $: activeGroup = pickSelectedGroup(issueGroups, selectedIssueKey);
  $: activeIssue = pickSelectedIssue(activeGroup, selectedIssueInstanceId);

  $: if (selectedReportFile !== lastSelectedReportFile) {
    lastSelectedReportFile = selectedReportFile;
    if (selectedReportFile) {
      shouldAutoSelectIssueGroup = true;
      refreshCurrentReport().catch((error) => {
        loadError = error instanceof Error ? error.message : String(error);
      });
    }
  }

  $: {
    const groups = issueGroups;
    if (!groups.length) {
      selectedIssueKey = null;
      selectedIssueInstanceId = null;
    } else if (
      shouldAutoSelectIssueGroup &&
      (!selectedIssueKey ||
        !groups.some((group) => group.key === selectedIssueKey))
    ) {
      selectedIssueKey = groups[0].key;
      selectedIssueInstanceId = groups[0].items[0]?.id ?? null;
      shouldAutoSelectIssueGroup = false;
    } else {
      const group = groups.find((value) => value.key === selectedIssueKey);
      if (
        group &&
        (!selectedIssueInstanceId ||
          !group.items.some((item) => item.id === selectedIssueInstanceId))
      ) {
        selectedIssueInstanceId = group.items[0]?.id ?? null;
      }
    }
  }
</script>

<svelte:head>
  <title>Arvalez Corpus Dashboard</title>
</svelte:head>

<div class="shell">
  <header class="hero panel">
    <div class="hero-copy">
      <div class="eyebrow">Arvalez Corpus Dashboard</div>
      <h1>Live Report Explorer</h1>
      <p>
        Watching <code>{reportDirectory || "REPORT_DIRECTORY"}</code> for new
        <code>apis-guru-*.json</code> reports.
      </p>
    </div>
    {#if currentReport}
      <div class="hero-stats">
        <div class="hero-stat lead">
          <span class="hero-stat-label">Support</span>
          <strong>{supportPercent(currentReport).toFixed(1)}%</strong>
          <span class="hero-stat-meta"
            >{currentReport.passed_specs}/{currentReport.total_specs} passed</span
          >
        </div>
        <div class="hero-stat">
          <span class="hero-stat-label">Failed</span>
          <strong>{currentReport.failed_specs}</strong>
          <span class="hero-stat-meta">specs</span>
        </div>
        <div class="hero-stat">
          <span class="hero-stat-label">Failures</span>
          <strong>{currentReport.summary?.total_failures ?? 0}</strong>
          <span class="hero-stat-meta">grouped events</span>
        </div>
        <div class="hero-stat">
          <span class="hero-stat-label">Warnings</span>
          <strong>{totalWarnings(currentReport)}</strong>
          <span class="hero-stat-meta">across specs</span>
        </div>
        <div class="hero-stat wide">
          <span class="hero-stat-label">Selected Report</span>
          <strong>{formatDate(currentReport.generated_at_unix_seconds)}</strong>
          {#if selectedReportFile}
            <code>{selectedReportFile}</code>
          {/if}
        </div>
      </div>
    {/if}
  </header>

  <div class="status-row">
    <span>{statusMessage}</span>
    {#if loadError}
      <span class="warning">{loadError}</span>
    {/if}
  </div>

  <section class="overview-layout">
    <section class="panel overview-panel">
      <div class="section-header">
        <div>
          <div class="eyebrow">Progression</div>
          <h2>Support Over Time</h2>
        </div>
        <div class="subtle">
          {#if selectedReportFile}
            Selected report:
            <code>{selectedReportFile}</code>
          {/if}
        </div>
      </div>

      {#if summaries.length === 0}
        <p>No reports loaded yet.</p>
      {:else}
        <svg
          class="trend-chart"
          viewBox="0 0 960 280"
          preserveAspectRatio="none"
          aria-label="Support trend"
        >
          <line class="axis" x1="40" y1="14" x2="40" y2="254"></line>
          <line class="axis" x1="40" y1="254" x2="948" y2="254"></line>
          <text x="40" y="18" class="axis-label">100%</text>
          <text x="40" y="272" class="axis-label">0%</text>
          <path class="series" d={graphPath(graphSeries)}></path>
          {#each graphSeries as point}
            <circle
              class:selected={selectedReportFile === point.report.file}
              class="point"
              cx={point.x}
              cy={point.y}
              r="5"
              role="button"
              tabindex="0"
              on:click={() => {
                shouldAutoSelectIssueGroup = true;
                selectedReportFile = point.report.file;
              }}
              on:keydown={(event) => {
                if (event.key === "Enter" || event.key === " ") {
                  event.preventDefault();
                  shouldAutoSelectIssueGroup = true;
                  selectedReportFile = point.report.file;
                }
              }}
            >
              <title
                >{formatDate(point.report.generated_at_unix_seconds)} · {supportPercent(
                  point.report,
                ).toFixed(1)}%</title
              >
            </circle>
          {/each}
        </svg>
      {/if}
    </section>

    <section class="panel overview-panel top-failures-panel">
      <div class="section-header">
        <div>
          <div class="eyebrow">Summary</div>
          <h2>Top Failure Groups</h2>
        </div>
      </div>
      {#if currentReport}
        <div class="top-failures">
          {#each Object.entries(currentReport.summary.by_kind_and_feature)
            .sort((left, right) => right[1] - left[1] || left[0].localeCompare(right[0]))
            .slice(0, 24) as [key, count]}
            <div class="top-failure-row">
              <code>{key}</code>
              <strong>{count}</strong>
            </div>
          {/each}
        </div>
      {:else}
        <p class="subtle">Waiting for a report selection.</p>
      {/if}
    </section>
  </section>

  <section class="panel issues-panel">
    <div class="section-header">
      <div>
        <div class="eyebrow">Failures</div>
        <h2>Grouped Issues</h2>
      </div>
      <input
        class="search"
        type="search"
        bind:value={issueSearch}
        placeholder="Filter kind, feature, pointer, spec…"
      />
    </div>

    {#if !currentReport}
      <p>Select a report to inspect its issues.</p>
    {:else}
      <div class="issue-accordion-list">
        {#each issueGroups as group}
          <details
            class="issue-family"
            open={selectedIssueKey === group.key}
            on:toggle={(event) => {
              if (event.currentTarget.open) {
                shouldAutoSelectIssueGroup = false;
                selectedIssueKey = group.key;
                selectedIssueInstanceId = group.items[0]?.id ?? null;
              } else if (selectedIssueKey === group.key) {
                shouldAutoSelectIssueGroup = false;
                selectedIssueKey = null;
                selectedIssueInstanceId = null;
              }
            }}
          >
            <summary class="issue-family-summary">
              <div class="issue-group-title">
                <span class="pill">{group.kind}</span>
                <span class="pill soft">{group.feature}</span>
              </div>
              <strong>{group.count}</strong>
            </summary>

            <div class="issue-family-body">
              <div class="detail-header">
                <div>
                  <div class="eyebrow">Issue family</div>
                  <h3>{group.kind}:{group.feature}</h3>
                </div>
                <span class="count-badge">{group.count} occurrence(s)</span>
              </div>

              {#if selectedIssueKey === group.key && activeIssue}
                <div class="issue-expanded-layout">
                  <div class="issue-main">
                    <div class="preview-card">
                      <div class="detail-grid">
                        <div>
                          <div class="detail-label">Spec</div>
                          <code>{activeIssue.spec}</code>
                        </div>
                        <div>
                          <div class="detail-label">Target</div>
                          <div>{activeIssue.target ?? "document-level"}</div>
                        </div>
                        <div>
                          <div class="detail-label">Path</div>
                          <code
                            >{activeIssue.failure.pointer ??
                              activeIssue.failure.schema_path ??
                              "—"}</code
                          >
                        </div>
                        {#if activeIssue.failure.line}
                          <div>
                            <div class="detail-label">Location</div>
                            <div>
                              line {activeIssue.failure.line}, column {activeIssue
                                .failure.column ?? "?"}
                            </div>
                          </div>
                        {/if}
                      </div>

                      {#if activeIssue.failure.source_preview}
                        {@const rawLines = activeIssue.failure.source_preview.split('\n')}
                        {@const previewLines = rawLines.at(-1) === '' ? rawLines.slice(0, -1) : rawLines}
                        {@const startLine = activeIssue.failure.line ?? 1}
                        <div class="detail-block">
                          <div class="detail-label">Source Preview</div>
                          <div class="code-preview">
                            {#each previewLines as line, i}
                              {@const isCaret = line.trimStart().startsWith('^')}
                              {@const lineOffset = previewLines.slice(0, i).filter((l) => !l.trimStart().startsWith('^')).length}
                              {@const lineNum = startLine + lineOffset}
                              <div
                                class="code-line"
                                class:highlighted={!isCaret && lineNum === startLine}
                                class:caret-line={isCaret}
                              >
                                <span class="ln">{isCaret ? '' : lineNum}</span>
                                <span class="lc">{line}</span>
                              </div>
                            {/each}
                          </div>
                        </div>
                      {/if}

                      {#if activeIssue.failure.note}
                        <div class="detail-block">
                          <div class="detail-label">Note</div>
                          <p>{activeIssue.failure.note}</p>
                        </div>
                      {/if}
                    </div>

                    <div class="detail-block">
                      <div class="detail-label">Message</div>
                      <pre>{activeIssue.failure.message}</pre>
                    </div>
                  </div>

                  <div class="issue-sidebar">
                    <div class="detail-label instance-label">Occurrences</div>
                    <div class="instance-list">
                      {#each group.items as item}
                        <button
                          type="button"
                          class:selected={selectedIssueInstanceId === item.id}
                          class="instance"
                          on:click={() => {
                            shouldAutoSelectIssueGroup = false;
                            selectedIssueKey = group.key;
                            selectedIssueInstanceId = item.id;
                          }}
                        >
                          <strong>{item.spec}</strong>
                          {#if item.target}
                            <span class="subtle">Target: {item.target}</span>
                          {/if}
                          {#if item.failure.pointer || item.failure.schema_path}
                            <code
                              >{item.failure.pointer ??
                                item.failure.schema_path}</code
                            >
                          {/if}
                        </button>
                      {/each}
                    </div>
                  </div>
                </div>
              {/if}
            </div>
          </details>
        {/each}

        {#if issueGroups.length === 0}
          <p class="subtle">No failures match this filter.</p>
        {/if}
      </div>
    {/if}
  </section>

  <section class="panel history-panel">
    <div class="section-header">
      <div>
        <div class="eyebrow">History</div>
        <h2>Loaded Reports</h2>
      </div>
    </div>
    <div class="history-list">
      {#each [...summaries].reverse() as report}
        <button
          type="button"
          class:selected={selectedReportFile === report.file}
          class="history-item"
          on:click={() => {
            shouldAutoSelectIssueGroup = true;
            selectedReportFile = report.file;
          }}
        >
          <strong>{formatDate(report.generated_at_unix_seconds)}</strong>
          <div class="history-meta">
            <span>{supportPercent(report).toFixed(1)}%</span>
            <span>{report.passed_specs}/{report.total_specs} passed</span>
          </div>
          <code>{report.file}</code>
        </button>
      {/each}
    </div>
  </section>
</div>

<style>
  :global(:root) {
    color-scheme: light;
    --bg: #f6f1e8;
    --panel: #fffdf7;
    --ink: #14212b;
    --muted: #566675;
    --line: #dfd2bd;
    --accent: #0f766e;
    --accent-soft: #d6f2ef;
    --accent-ink: #0c615a;
    --danger: #b42318;
    --shadow: 0 14px 38px rgba(20, 33, 43, 0.08);
  }

  :global(body) {
    margin: 0;
    font-family: "Iowan Old Style", "Palatino Linotype", Georgia, serif;
    color: var(--ink);
    background: radial-gradient(
        circle at top left,
        rgba(15, 118, 110, 0.14),
        transparent 30rem
      ),
      radial-gradient(
        circle at bottom right,
        rgba(180, 35, 24, 0.08),
        transparent 25rem
      ),
      linear-gradient(180deg, #fbf6ec 0%, var(--bg) 100%);
  }

  :global(*) {
    box-sizing: border-box;
  }

  .shell {
    max-width: 1440px;
    margin: 0 auto;
    padding: 1.5rem;
  }

  .panel {
    background: color-mix(in srgb, var(--panel) 94%, white);
    border: 1px solid var(--line);
    border-radius: 1.25rem;
    box-shadow: var(--shadow);
    padding: 1rem 1.1rem;
  }

  .hero {
    display: grid;
    grid-template-columns: minmax(0, 1.1fr) minmax(22rem, 0.9fr);
    gap: 1rem;
    align-items: stretch;
    margin-bottom: 1rem;
  }

  .hero-copy {
    min-width: 0;
  }

  .hero-stats {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 0.75rem;
    min-width: 0;
  }

  .hero-stat {
    display: grid;
    gap: 0.2rem;
    padding: 0.9rem 1rem;
    border: 1px solid var(--line);
    border-radius: 1rem;
    background: rgba(255, 255, 255, 0.66);
    min-width: 0;
  }

  .hero-stat.lead {
    background: color-mix(in srgb, var(--accent-soft) 78%, white);
  }

  .hero-stat.wide {
    grid-column: 1 / -1;
  }

  .hero-stat-label {
    text-transform: uppercase;
    letter-spacing: 0.08em;
    font-size: 0.72rem;
    color: var(--muted);
  }

  .hero-stat strong {
    font-size: 1.35rem;
    line-height: 1.1;
  }

  .hero-stat-meta {
    color: var(--muted);
    font-size: 0.88rem;
  }

  .hero-stat code {
    margin-top: 0.15rem;
  }

  .hero h1,
  h2,
  h3 {
    margin: 0;
  }

  .hero p,
  .subtle,
  .detail-block p {
    color: var(--muted);
  }

  .status-row {
    display: flex;
    justify-content: space-between;
    gap: 1rem;
    margin-bottom: 1rem;
    color: var(--muted);
  }

  .warning {
    color: var(--danger);
  }

  .eyebrow,
  .detail-label {
    text-transform: uppercase;
    letter-spacing: 0.08em;
    font-size: 0.72rem;
    color: var(--muted);
  }

  .section-header,
  .detail-header {
    display: flex;
    justify-content: space-between;
    gap: 1rem;
    align-items: center;
    margin-bottom: 0.9rem;
  }

  .overview-layout {
    display: grid;
    grid-template-columns: minmax(0, 1.8fr) minmax(18rem, 24rem);
    gap: 1rem;
    align-items: stretch;
  }

  .overview-layout > * {
    min-width: 0;
  }

  .overview-panel {
    height: 24rem;
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .top-failures-panel {
    overflow: hidden;
  }

  .trend-chart {
    width: 100%;
    height: 280px;
    display: block;
    flex: 1 1 auto;
  }

  .axis {
    stroke: #ccbda3;
    stroke-width: 1;
  }

  .axis-label {
    fill: var(--muted);
    font-size: 12px;
  }

  .series {
    fill: none;
    stroke: var(--accent);
    stroke-width: 3;
    stroke-linecap: round;
    stroke-linejoin: round;
  }

  .point {
    fill: #fff;
    stroke: var(--accent);
    stroke-width: 2;
    cursor: pointer;
    transition:
      fill 0.15s ease,
      stroke-width 0.15s ease,
      stroke 0.15s ease;
  }

  .point:hover {
    fill: var(--accent-soft);
    stroke-width: 3;
  }

  .point.selected {
    fill: var(--accent);
    stroke: #fff;
    stroke-width: 3;
  }

  .search {
    width: min(26rem, 100%);
    padding: 0.7rem 0.85rem;
    border-radius: 999px;
    border: 1px solid var(--line);
    background: rgba(255, 255, 255, 0.8);
    color: var(--ink);
  }

  .issues-panel {
    margin-top: 1rem;
  }

  .issue-accordion-list,
  .instance-list,
  .history-list,
  .top-failures {
    display: grid;
    gap: 0.6rem;
  }

  .top-failures {
    flex: 1 1 auto;
    min-height: 0;
    overflow: auto;
    padding-right: 0.2rem;
  }

  .issue-family,
  .instance,
  .history-item {
    border: 1px solid var(--line);
    background: rgba(255, 255, 255, 0.72);
    color: var(--ink);
    border-radius: 1rem;
    transition:
      border-color 0.16s ease,
      transform 0.16s ease,
      background 0.16s ease;
  }

  .issue-family:hover,
  .instance:hover,
  .history-item:hover,
  .instance.selected,
  .history-item.selected {
    border-color: color-mix(in srgb, var(--accent) 50%, var(--line));
    background: rgba(214, 242, 239, 0.6);
    transform: translateY(-1px);
  }

  .issue-family[open] {
    background: rgba(214, 242, 239, 0.28);
    border-color: color-mix(in srgb, var(--accent) 40%, var(--line));
  }

  .issue-family-summary {
    list-style: none;
    display: flex;
    justify-content: space-between;
    gap: 0.75rem;
    align-items: center;
    padding: 0.85rem 0.95rem;
    cursor: pointer;
  }

  .issue-family-summary::-webkit-details-marker {
    display: none;
  }

  .issue-family-body {
    padding: 0 0.95rem 0.95rem;
    border-top: 1px solid rgba(223, 210, 189, 0.8);
    min-width: 0;
  }

  .instance,
  .history-item {
    width: 100%;
    text-align: left;
    padding: 0.8rem 0.9rem;
    cursor: pointer;
  }

  .issue-group-title {
    display: flex;
    flex-wrap: wrap;
    gap: 0.35rem;
    min-width: 0;
  }

  .issue-group-title > * {
    max-width: 100%;
  }

  .pill,
  .count-badge {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    padding: 0.24rem 0.58rem;
    border-radius: 999px;
    font-size: 0.78rem;
    background: var(--accent-soft);
    color: var(--accent-ink);
  }

  .pill.soft {
    background: rgba(255, 255, 255, 0.82);
    color: var(--muted);
    border: 1px solid var(--line);
  }

  .detail-grid {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 0.8rem 1rem;
    margin: 1rem 0;
  }

  .issue-family-body h3,
  .issue-family-body code,
  .issue-family-body pre,
  .top-failure-row code,
  .history-item code,
  .instance code {
    min-width: 0;
    overflow-wrap: anywhere;
    word-break: break-word;
  }

  .detail-block {
    margin-top: 1rem;
  }

  .preview-card {
    margin-bottom: 1rem;
  }

  .issue-expanded-layout {
    display: grid;
    grid-template-columns: 3fr 1fr;
    gap: 1.25rem;
    align-items: start;
  }

  .issue-sidebar {
    min-width: 0;
    position: sticky;
    top: 1rem;
  }

  .issue-sidebar .instance-list {
    max-height: 28rem;
    overflow-y: auto;
  }

  .instance-label {
    display: block;
    margin-bottom: 0.55rem;
  }

  .history-panel {
    margin-top: 1rem;
  }

  .history-meta {
    display: flex;
    justify-content: space-between;
    gap: 0.75rem;
    color: var(--muted);
    margin: 0.35rem 0 0.4rem;
  }

  .top-failure-row {
    display: flex;
    justify-content: space-between;
    gap: 1rem;
    align-items: start;
    padding: 0.55rem 0.05rem;
    border-bottom: 1px solid var(--line);
    min-width: 0;
  }

  .top-failure-row > :first-child {
    flex: 1 1 auto;
    min-width: 0;
  }

  .top-failure-row > strong {
    flex: 0 0 auto;
    white-space: nowrap;
  }

  code,
  pre {
    font-family: "SFMono-Regular", ui-monospace, Menlo, monospace;
    font-size: 0.85rem;
  }

  code {
    word-break: break-word;
  }

  pre {
    white-space: pre-wrap;
    margin: 0;
    padding: 0.9rem 1rem;
    border-radius: 1rem;
    background: #f3ece1;
    border: 1px solid var(--line);
    color: var(--ink);
  }

  .code-preview {
    font-family: "SFMono-Regular", ui-monospace, Menlo, monospace;
    font-size: 0.85rem;
    border-radius: 1rem;
    background: #f3ece1;
    border: 1px solid var(--line);
    color: var(--ink);
    overflow-x: auto;
    padding: 0.5rem 0;
  }

  .code-line {
    display: flex;
    align-items: baseline;
    line-height: 1.6;
    padding: 0 1rem 0 0;
  }

  .code-line.highlighted {
    background: rgba(200, 100, 20, 0.13);
    outline: 1px solid rgba(200, 100, 20, 0.25);
    outline-offset: -1px;
  }

  .code-line.caret-line .lc {
    color: #b94a16;
  }

  .code-line .ln {
    min-width: 3.5ch;
    padding: 0 0.75rem;
    color: #b0a090;
    text-align: right;
    flex-shrink: 0;
    user-select: none;
    border-right: 1px solid var(--line);
    margin-right: 1rem;
  }

  .code-line .lc {
    white-space: pre-wrap;
    word-break: break-all;
    flex: 1;
  }

  @media (max-width: 1100px) {
    .overview-layout,
    .hero {
      display: grid;
      grid-template-columns: 1fr;
    }

    .overview-panel {
      height: auto;
    }
  }

  @media (max-width: 720px) {
    .shell {
      padding: 1rem;
    }

    .section-header,
    .detail-header,
    .status-row {
      align-items: start;
      flex-direction: column;
    }

    .detail-grid,
    .issue-expanded-layout {
      grid-template-columns: 1fr;
    }

    .issue-sidebar {
      position: static;
    }

    .issue-sidebar .instance-list {
      max-height: none;
    }

    .search {
      width: 100%;
    }
  }
</style>
