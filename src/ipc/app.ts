import { getVersion } from "@tauri-apps/api/app";
import { MOCK_MODE } from "./mock";

/** Current app version. Falls back to the injected build version in mock mode. */
export async function appVersion(): Promise<string> {
  if (MOCK_MODE) return import.meta.env.VITE_APP_VERSION ?? "dev";
  return getVersion();
}
