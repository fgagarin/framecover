"use strict";

// Release filenames include a version; resolve them without rebuilding the site.
async function loadDownloads() {
  const status = document.getElementById("release-status");
  try {
    const response = await fetch(
      "https://api.github.com/repos/fgagarin/video-thumbnailer/releases/latest",
      { headers: { Accept: "application/vnd.github+json" }, signal: AbortSignal.timeout(8000) },
    );
    if (response.status === 404) {
      status.textContent = "No stable release has been published yet. Builds will be available on GitHub after the first release.";
      return;
    }
    if (!response.ok) throw new Error("Release unavailable");
    const release = await response.json();
    const extensions = { windows: "exe", macos: "tar.gz", linux: "tar.gz" };
    let resolved = 0;
    for (const link of document.querySelectorAll("[data-platform]")) {
      const platform = link.dataset.platform;
      const suffix = `-${platform}.${extensions[platform]}`;
      const asset = release.assets.find(
        (item) => item.name.startsWith("video-thumbnailer-") && item.name.endsWith(suffix),
      );
      if (!asset) continue;
      // Keep API data out of HTML and restrict download navigation to this repo.
      const url = new URL(asset.browser_download_url);
      if (url.origin !== "https://github.com" ||
          !url.pathname.startsWith("/fgagarin/video-thumbnailer/releases/download/")) continue;
      link.href = url.href;
      resolved += 1;
    }
    status.textContent = resolved === 3
      ? `Latest stable release: ${release.tag_name}. The buttons download the build for your chosen OS.`
      : "Some builds are not available yet. Their buttons will open the latest release page.";
  } catch {
    status.textContent = "Could not check the release files. The buttons will open the latest stable release on GitHub — choose the build for your OS.";
  }
}

loadDownloads();
