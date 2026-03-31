import { getVersion } from "@tauri-apps/api/app";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { ask, message } from "@tauri-apps/plugin-dialog";
import { relaunch } from "@tauri-apps/plugin-process";
import { isWindowsPlatform } from "./platform";

export type UpdateCheckMode = "background" | "interactive";

export async function checkForUpdates(mode: UpdateCheckMode = "interactive"): Promise<void> {
  const interactive = mode === "interactive";

  try {
    const currentVersion = await getVersion().catch(() => null);
    const update: Update | null = await check();

    if (!update) {
      console.log(
        `[Updater] No update available${currentVersion ? ` (current: ${currentVersion})` : ""}`
      );

      if (interactive) {
        await message(
          currentVersion
            ? `Copi ${currentVersion} is already up to date.`
            : "Copi is already up to date.",
          {
            title: "No Updates Found",
            kind: "info",
          }
        );
      }

      return;
    }

    console.log(`[Updater] Update available: ${update.version}`);

    const prompt = [
      currentVersion ? `Current version: ${currentVersion}` : "Current version: Unknown",
      `Latest version: ${update.version}`,
      "\nDownload and install now?",
    ]
      .filter(Boolean)
      .join("\n");

    const userConfirmed = await ask(prompt, {
      title: "Update Available",
      kind: "info",
    });

    if (!userConfirmed) return;

    await downloadAndInstall(update);
  } catch (error) {
    console.error("[Updater] Failed:", error);
    if (interactive) {
      const errorMessageRaw = error instanceof Error ? error.message : String(error);
      const errorMessage = isWindowsPlatform && errorMessageRaw.includes("windows-x86_64")
        ? "No compatible Windows update package is available for this build channel yet."
        : errorMessageRaw;
      await message(`Update check failed: ${errorMessage}`, {
        title: "Update Error",
        kind: "error",
      });
    }
  }
}

async function downloadAndInstall(update: Update): Promise<void> {
  let downloaded = 0;
  let contentLength = 0;

  await update.downloadAndInstall((event) => {
    switch (event.event) {
      case "Started":
        contentLength = event.data.contentLength ?? 0;
        console.log(`[Updater] Download started: ${contentLength} bytes`);
        break;
      case "Progress":
        downloaded += event.data.chunkLength;
        break;
      case "Finished":
        console.log("[Updater] Download finished");
        break;
    }
  });

  const shouldRelaunch = await ask(
    `Copi ${update.version} is ready. Restart now to apply the update?`,
    { title: "Restart Required", kind: "info" }
  );

  if (shouldRelaunch) {
    await relaunch();
  }
}
