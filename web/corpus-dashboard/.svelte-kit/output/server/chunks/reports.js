import { readdir, readFile } from "node:fs/promises";
import { watch } from "node:fs";
import { resolve } from "node:path";
const REPORT_FILE_RE = /^apis-guru-\d+\.json$/;
const SSE_ENCODER = new TextEncoder();
const SSE_HEADERS = {
  "cache-control": "no-cache",
  connection: "keep-alive",
  "content-type": "text/event-stream"
};
let watcher = null;
let watchedDirectory = null;
let broadcastTimer = null;
const subscribers = /* @__PURE__ */ new Set();
function getReportDirectory() {
  return process.env.REPORT_DIRECTORY ?? resolve(process.cwd(), "../../reports/apis-guru");
}
async function listReportSummaries() {
  const reportDirectory = getReportDirectory();
  let entries = [];
  try {
    entries = await readdir(reportDirectory, { withFileTypes: true });
  } catch (error) {
    if (error && typeof error === "object" && "code" in error && error.code === "ENOENT") {
      return {
        reportDirectory,
        reports: [],
        latestReportFile: null
      };
    }
    throw error;
  }
  const reports = [];
  for (const entry of entries) {
    if (!entry.isFile() || !REPORT_FILE_RE.test(entry.name)) {
      continue;
    }
    const report = await loadReport(entry.name);
    reports.push({
      file: entry.name,
      generated_at_unix_seconds: report.generated_at_unix_seconds,
      total_specs: report.total_specs,
      passed_specs: report.passed_specs,
      failed_specs: report.failed_specs,
      summary: report.summary
    });
  }
  reports.sort(
    (left, right) => left.generated_at_unix_seconds - right.generated_at_unix_seconds || left.file.localeCompare(right.file)
  );
  return {
    reportDirectory,
    reports,
    latestReportFile: reports.at(-1)?.file ?? null
  };
}
async function loadReport(file) {
  if (!REPORT_FILE_RE.test(file)) {
    throw new Error(`Invalid report filename: ${file}`);
  }
  const path = resolve(getReportDirectory(), file);
  const raw = await readFile(path, "utf8");
  return JSON.parse(raw);
}
function createReportsEventStream() {
  ensureWatcher();
  return new ReadableStream({
    start(controller) {
      const subscriber = () => {
        controller.enqueue(sseMessage({ type: "updated" }));
      };
      subscribers.add(subscriber);
      controller.enqueue(sseMessage({ type: "ready" }));
      const keepAlive = setInterval(() => {
        controller.enqueue(SSE_ENCODER.encode(": keep-alive\n\n"));
      }, 15e3);
      controller.enqueue(sseMessage({ type: "directory", reportDirectory: getReportDirectory() }));
      this.cleanup = () => {
        clearInterval(keepAlive);
        subscribers.delete(subscriber);
      };
    },
    cancel() {
      this.cleanup?.();
    }
  });
}
function ensureWatcher() {
  const reportDirectory = getReportDirectory();
  if (watcher && watchedDirectory === reportDirectory) {
    return;
  }
  if (watcher) {
    watcher.close();
    watcher = null;
  }
  watchedDirectory = reportDirectory;
  try {
    watcher = watch(reportDirectory, { persistent: false }, (_eventType, fileName) => {
      if (typeof fileName !== "string" || !REPORT_FILE_RE.test(fileName)) {
        return;
      }
      scheduleBroadcast();
    });
  } catch (_error) {
    watcher = null;
  }
}
function scheduleBroadcast() {
  clearTimeout(broadcastTimer);
  broadcastTimer = setTimeout(() => {
    for (const subscriber of subscribers) {
      subscriber();
    }
  }, 150);
}
function sseMessage(payload) {
  return SSE_ENCODER.encode(`data: ${JSON.stringify(payload)}

`);
}
export {
  SSE_HEADERS as S,
  loadReport as a,
  createReportsEventStream as c,
  listReportSummaries as l
};
