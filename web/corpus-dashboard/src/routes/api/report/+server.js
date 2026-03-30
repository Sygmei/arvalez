import { json } from "@sveltejs/kit";
import { loadReport } from "$lib/server/reports";

export async function GET({ url }) {
  const file = url.searchParams.get("file");
  if (!file) {
    return json({ error: "Missing `file` query parameter." }, { status: 400 });
  }

  try {
    return json(await loadReport(file));
  } catch (error) {
    return json(
      { error: error instanceof Error ? error.message : String(error) },
      { status: 404 }
    );
  }
}
