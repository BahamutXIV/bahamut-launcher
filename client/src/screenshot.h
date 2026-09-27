#pragma once

#include <d3d9.h>

#include <filesystem>

enum class ScreenshotFormat
{
    Png,
    Bmp,
};

void ConfigureScreenshotCapture(const std::filesystem::path& directory,
                                ScreenshotFormat             format,
                                bool                         hideOverlays);
bool IsScreenshotCaptureConfigured();
bool RequestScreenshotCapture();
void CancelScreenshotCapture();
bool PendingScreenshotHidesOverlays();
bool CapturePendingScreenshot(IDirect3DDevice9* device);
