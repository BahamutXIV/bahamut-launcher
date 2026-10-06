#include "overlay.h"
#include "addon_host.h"
#include "input.h"
#include "native_plugin_host.h"
#include "player_state.h"
#include "render_boundary.h"
#include "targetlines_probe.h"

#include <imgui.h>
#include <imgui_impl_dx9.h>
#include <imgui_impl_win32.h>

#include <algorithm>
#include <array>
#include <cfloat>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <string>
#include <string_view>

namespace
{

constexpr std::size_t kMaximumRenderTargets   = 4;
volatile LONG         gBackBufferTestRecorded = 0;
ImFont*               gZoneRegionFont         = nullptr;
ImFont*               gZoneAreaFont           = nullptr;

bool HasTestFault(const wchar_t* name)
{
    wchar_t     value[8]{};
    const DWORD count = GetEnvironmentVariableW(name, value, ARRAYSIZE(value));
    return count != 0 && count < ARRAYSIZE(value) && value[0] != L'0';
}

void RecordTestResult(std::string_view text)
{
    wchar_t     path[32768]{};
    const DWORD count = GetEnvironmentVariableW(L"BAHAMUT_TEST_OVERLAY_RESULT_FILE",
                                                path,
                                                ARRAYSIZE(path));
    if (count == 0 || count >= ARRAYSIZE(path))
    {
        return;
    }
    std::ofstream file(path, std::ios::trunc);
    if (file)
    {
        file << text;
    }
}

class CompleteDeviceState
{
public:
    explicit CompleteDeviceState(IDirect3DDevice9* device)
    : device_(device)
    {
        captured_ = Capture();
    }

    ~CompleteDeviceState()
    {
        if (captured_ && !restored_)
        {
            if (!Restore())
            {
                DisableOverlay();
                RecordTestResult(IsOverlayVisible()
                                     ? "overlay_restore=failed\nvisible=1\n"
                                     : "overlay_restore=failed\nvisible=0\n");
            }
        }
        ReleaseReferences();
    }

    CompleteDeviceState(const CompleteDeviceState&)            = delete;
    CompleteDeviceState& operator=(const CompleteDeviceState&) = delete;

    bool IsCaptured() const
    {
        return captured_;
    }

    bool BindBackBuffer()
    {
        IDirect3DSurface9* backBuffer = nullptr;
        if (FAILED(device_->GetBackBuffer(
                0, 0, D3DBACKBUFFER_TYPE_MONO, &backBuffer)) ||
            backBuffer == nullptr)
        {
            return false;
        }
        bool ok = SUCCEEDED(device_->SetDepthStencilSurface(nullptr)) && SUCCEEDED(device_->SetRenderTarget(0, backBuffer));
        backBuffer->Release();
        for (std::size_t index = 1; index < targetCount_; ++index)
        {
            ok = SUCCEEDED(device_->SetRenderTarget(
                     static_cast<DWORD>(index), nullptr)) &&
                 ok;
        }
        return ok;
    }

    bool IsBackBufferBound() const
    {
        IDirect3DSurface9* backBuffer   = nullptr;
        IDirect3DSurface9* renderTarget = nullptr;
        const bool         available    = SUCCEEDED(device_->GetBackBuffer(
                                              0, 0, D3DBACKBUFFER_TYPE_MONO, &backBuffer)) &&
                                          backBuffer != nullptr && SUCCEEDED(device_->GetRenderTarget(0, &renderTarget)) && renderTarget != nullptr;
        const bool         matches      = available && backBuffer == renderTarget;
        if (renderTarget != nullptr)
            renderTarget->Release();
        if (backBuffer != nullptr)
            backBuffer->Release();
        return matches;
    }

    bool Restore()
    {
        if (!captured_ || restored_)
        {
            return restored_;
        }
        bool ok = SUCCEEDED(stateBlock_->Apply());
        for (std::size_t index = 0; index < targetCount_; ++index)
        {
            if (renderTargetCaptured_[index])
            {
                ok = SUCCEEDED(device_->SetRenderTarget(
                         static_cast<DWORD>(index), renderTargets_[index])) &&
                     ok;
            }
        }
        if (depthStencilCaptured_)
        {
            ok = SUCCEEDED(device_->SetDepthStencilSurface(depthStencil_)) && ok;
        }
        // SetRenderTarget resets the viewport, so dependent state returns last.
        ok = SUCCEEDED(device_->SetViewport(&viewport_)) && ok;
        ok = SUCCEEDED(device_->SetScissorRect(&scissor_)) && ok;
        ok = SUCCEEDED(device_->SetTransform(D3DTS_WORLD, &world_)) && ok;
        ok = SUCCEEDED(device_->SetTransform(D3DTS_VIEW, &view_)) && ok;
        ok = SUCCEEDED(device_->SetTransform(D3DTS_PROJECTION, &projection_)) && ok;
        if (HasTestFault(L"BAHAMUT_TEST_OVERLAY_RESTORE_FAILURE"))
        {
            ok = false;
        }
        restored_ = ok;
        return ok;
    }

private:
    bool Capture()
    {
        if (FAILED(device_->CreateStateBlock(D3DSBT_ALL, &stateBlock_)) || stateBlock_ == nullptr || FAILED(stateBlock_->Capture()) || FAILED(device_->GetTransform(D3DTS_WORLD, &world_)) || FAILED(device_->GetTransform(D3DTS_VIEW, &view_)) || FAILED(device_->GetTransform(D3DTS_PROJECTION, &projection_)) || FAILED(device_->GetViewport(&viewport_)) || FAILED(device_->GetScissorRect(&scissor_)))
        {
            return false;
        }

        D3DCAPS9 capabilities{};
        if (FAILED(device_->GetDeviceCaps(&capabilities)))
        {
            return false;
        }
        targetCount_ = std::min<std::size_t>(capabilities.NumSimultaneousRTs,
                                             kMaximumRenderTargets);
        for (std::size_t index = 0; index < targetCount_; ++index)
        {
            const HRESULT result = device_->GetRenderTarget(
                static_cast<DWORD>(index), &renderTargets_[index]);
            if (result == D3DERR_NOTFOUND)
            {
                renderTargetCaptured_[index] = false;
            }
            else if (FAILED(result))
            {
                return false;
            }
            else
            {
                renderTargetCaptured_[index] = true;
            }
        }
        const HRESULT depthResult = device_->GetDepthStencilSurface(&depthStencil_);
        depthStencilCaptured_     = SUCCEEDED(depthResult) || depthResult == D3DERR_NOTFOUND;
        return depthStencilCaptured_;
    }

    void ReleaseReferences()
    {
        for (IDirect3DSurface9*& target : renderTargets_)
        {
            if (target != nullptr)
            {
                target->Release();
                target = nullptr;
            }
        }
        if (depthStencil_ != nullptr)
        {
            depthStencil_->Release();
            depthStencil_ = nullptr;
        }
        if (stateBlock_ != nullptr)
        {
            stateBlock_->Release();
            stateBlock_ = nullptr;
        }
    }

    IDirect3DDevice9*                                     device_     = nullptr;
    IDirect3DStateBlock9*                                 stateBlock_ = nullptr;
    std::array<IDirect3DSurface9*, kMaximumRenderTargets> renderTargets_{};
    std::array<bool, kMaximumRenderTargets>               renderTargetCaptured_{};
    std::size_t                                           targetCount_  = 0;
    IDirect3DSurface9*                                    depthStencil_ = nullptr;
    D3DMATRIX                                             world_{};
    D3DMATRIX                                             view_{};
    D3DMATRIX                                             projection_{};
    D3DVIEWPORT9                                          viewport_{};
    RECT                                                  scissor_{};
    bool                                                  depthStencilCaptured_ = false;
    bool                                                  captured_             = false;
    bool                                                  restored_             = false;
};

void RecordBackBufferTestResult(const CompleteDeviceState& state)
{
    if (InterlockedCompareExchange(&gBackBufferTestRecorded, 1, 0) != 0)
    {
        return;
    }
    wchar_t     path[32768]{};
    const DWORD count = GetEnvironmentVariableW(
        L"BAHAMUT_TEST_OVERLAY_TARGET_RESULT_FILE", path, ARRAYSIZE(path));
    if (count == 0 || count >= ARRAYSIZE(path))
    {
        return;
    }
    std::ofstream file(path, std::ios::trunc);
    if (file)
    {
        file << (state.IsBackBufferBound()
                     ? "overlay_target=backbuffer\n"
                     : "overlay_target=other\n");
    }
}

bool          gInitialized          = false;
bool          gBackendReady         = false;
LONG          gBackendRetryAttempt  = 0;
HWND          gWindow               = nullptr;
volatile LONG gOverlayDrawing       = 0;
volatile LONG gFillModeTestRecorded = 0;
// The first successfully hooked device owns all overlay state for the process.
// Other D3D9 device callbacks pass through without touching ImGui or telemetry.
IDirect3DDevice9* gOverlayDevice = nullptr;
LARGE_INTEGER     gLastAddonFrame{};
float             gNextAddonWindowY = 0.0f;
std::string       gDraggedAddonId;
bool              gSaveLayoutRequested = false;

std::filesystem::path& OverlayLayoutPath()
{
    static std::filesystem::path path;
    return path;
}

void SaveOverlayLayout()
{
    if (OverlayLayoutPath().empty() || ImGui::GetCurrentContext() == nullptr)
    {
        return;
    }
    std::size_t   size     = 0;
    const char*   settings = ImGui::SaveIniSettingsToMemory(&size);
    std::ofstream file(
        OverlayLayoutPath(), std::ios::binary | std::ios::trunc);
    if (file && settings != nullptr)
    {
        file.write(settings, static_cast<std::streamsize>(size));
    }
}

bool InitializeOverlay(IDirect3DDevice9* device)
{
    if (!IsOverlayDevice(device))
    {
        return false;
    }
    IMGUI_CHECKVERSION();
    gZoneRegionFont = nullptr;
    gZoneAreaFont   = nullptr;
    ImGui::CreateContext();
    ImGuiIO& io    = ImGui::GetIO();
    io.IniFilename = nullptr;
    io.LogFilename = nullptr;
    io.ConfigFlags |= ImGuiConfigFlags_NavEnableKeyboard | ImGuiConfigFlags_NavEnableGamepad;
    ImGui::StyleColorsDark();
    io.Fonts->AddFontDefault();
    char       windowsDirectory[MAX_PATH]{};
    const UINT directoryLength = GetWindowsDirectoryA(windowsDirectory, MAX_PATH);
    if (directoryLength != 0 && directoryLength < MAX_PATH)
    {
        const std::string fontDirectory  = std::string(windowsDirectory) + "\\Fonts\\";
        const std::string regionFontPath = fontDirectory + "georgiab.ttf";
        const std::string areaFontPath   = fontDirectory + "georgiaz.ttf";
        if (std::filesystem::exists(regionFontPath))
        {
            gZoneRegionFont = io.Fonts->AddFontFromFileTTF(regionFontPath.c_str(), 13.0f);
        }
        if (std::filesystem::exists(areaFontPath))
        {
            gZoneAreaFont = io.Fonts->AddFontFromFileTTF(areaFontPath.c_str(), 30.0f);
        }
    }
    if (!OverlayLayoutPath().empty())
    {
        std::ifstream     file(OverlayLayoutPath(), std::ios::binary);
        const std::string settings{
            std::istreambuf_iterator<char>(file), std::istreambuf_iterator<char>()
        };
        if (!settings.empty())
        {
            ImGui::LoadIniSettingsFromMemory(settings.data(), settings.size());
        }
    }
    if (gWindow == nullptr || !ImGui_ImplWin32_Init(gWindow) || !ImGui_ImplDX9_Init(device))
    {
        if (io.BackendPlatformUserData != nullptr)
            ImGui_ImplWin32_Shutdown();
        ImGui::DestroyContext();
        return false;
    }
    gInitialized         = true;
    gBackendReady        = true;
    gBackendRetryAttempt = 0;
    return true;
}

void DrawAddonWindow(void* context, const char* addonId, const char* title, const char* text, bool locked)
{
    UNREFERENCED_PARAMETER(context);
    const ImGuiViewport*   viewport   = ImGui::GetMainViewport();
    const bool             headerless = title[0] == '\0';
    const std::string_view id(addonId);
    if (id == "combatparser")
    {
        ImGui::SetNextWindowPos(ImVec2(
                                    viewport->WorkPos.x + 28.0f,
                                    viewport->WorkPos.y + viewport->WorkSize.y * 0.5f),
                                ImGuiCond_FirstUseEver,
                                ImVec2(0.0f, 0.5f));
        ImGui::SetNextWindowSizeConstraints(ImVec2(380.0f, 0.0f), ImVec2(FLT_MAX, FLT_MAX));
    }
    else if (id == "targethp")
    {
        ImGui::SetNextWindowPos(ImVec2(
                                    viewport->WorkPos.x + viewport->WorkSize.x * 0.25f,
                                    viewport->WorkPos.y + 16.0f),
                                ImGuiCond_FirstUseEver,
                                ImVec2(0.5f, 0.0f));
    }
    else if (id == "distance")
    {
        ImGui::SetNextWindowPos(ImVec2(
                                    viewport->WorkPos.x + viewport->WorkSize.x - 28.0f,
                                    viewport->WorkPos.y + viewport->WorkSize.y * 0.5f),
                                ImGuiCond_FirstUseEver,
                                ImVec2(1.0f, 0.5f));
    }
    else if (headerless)
    {
        ImGui::SetNextWindowPos(ImVec2(
                                    viewport->WorkPos.x + 28.0f,
                                    viewport->WorkPos.y + viewport->WorkSize.y - 8.0f),
                                ImGuiCond_FirstUseEver,
                                ImVec2(0.0f, 1.0f));
    }
    else
    {
        ImGui::SetNextWindowPos(ImVec2(
                                    viewport->WorkPos.x + viewport->WorkSize.x - 16.0f,
                                    gNextAddonWindowY),
                                ImGuiCond_FirstUseEver,
                                ImVec2(1.0f, 0.0f));
    }
    ImGui::SetNextWindowBgAlpha(0.82f);
    const std::string windowName = std::string(title) + "###bahamut-addon-" + addonId;
    ImGuiWindowFlags  flags      = ImGuiWindowFlags_AlwaysAutoResize | ImGuiWindowFlags_NoCollapse | ImGuiWindowFlags_NoMove;
    if (headerless)
    {
        flags |= ImGuiWindowFlags_NoTitleBar;
    }
    if (locked || !ImGui::GetIO().KeyShift)
    {
        flags |= ImGuiWindowFlags_NoInputs;
    }
    if (ImGui::Begin(windowName.c_str(), nullptr, flags))
    {
        if (text[0] != '\0')
        {
            ImGui::TextUnformatted(text);
        }
    }
    const ImGuiIO& io = ImGui::GetIO();
    if (!locked && io.KeyShift && ImGui::IsWindowHovered(ImGuiHoveredFlags_AllowWhenBlockedByActiveItem) && ImGui::IsMouseClicked(ImGuiMouseButton_Left))
    {
        gDraggedAddonId = addonId;
    }
    if (locked && gDraggedAddonId == addonId)
    {
        gDraggedAddonId.clear();
    }
    else if (gDraggedAddonId == addonId)
    {
        if (!io.KeyShift || !ImGui::IsMouseDown(ImGuiMouseButton_Left))
        {
            gDraggedAddonId.clear();
            gSaveLayoutRequested = true;
        }
        else
        {
            const ImVec2 position = ImGui::GetWindowPos();
            ImGui::SetWindowPos(ImVec2(
                position.x + io.MouseDelta.x, position.y + io.MouseDelta.y));
        }
    }
    if (!headerless)
    {
        gNextAddonWindowY += ImGui::GetWindowSize().y + ImGui::GetStyle().ItemSpacing.y;
    }
    ImGui::End();
}

ImU32 ParseMeterColor(std::string_view value)
{
    const auto hexDigit = [](unsigned char character)
    {
        if (character >= '0' && character <= '9')
        {
            return static_cast<unsigned int>(character - '0');
        }
        if (character >= 'a' && character <= 'f')
        {
            return static_cast<unsigned int>(character - 'a' + 10);
        }
        if (character >= 'A' && character <= 'F')
        {
            return static_cast<unsigned int>(character - 'A' + 10);
        }
        return 0u;
    };
    if (value.size() != 7 || value[0] != '#')
    {
        return IM_COL32(160, 168, 180, 255);
    }
    const auto channel = [&](std::size_t index)
    {
        return hexDigit(static_cast<unsigned char>(value[index])) * 16u +
               hexDigit(static_cast<unsigned char>(value[index + 1u]));
    };
    return IM_COL32(channel(1), channel(3), channel(5), 255);
}

void DrawAddonCombatMeter(void* context, const char* addonId, const char* mode, const AddonCombatMeterRow* rows, std::size_t rowCount, bool locked, bool incomplete)
{
    UNREFERENCED_PARAMETER(context);
    const ImGuiViewport* viewport = ImGui::GetMainViewport();
    ImGui::SetNextWindowPos(ImVec2(viewport->WorkPos.x + 28.0f,
                                   viewport->WorkPos.y + viewport->WorkSize.y * 0.5f),
                            ImGuiCond_FirstUseEver,
                            ImVec2(0.0f, 0.5f));
    ImGui::SetNextWindowSize(ImVec2(392.0f, 0.0f), ImGuiCond_FirstUseEver);
    ImGui::SetNextWindowSizeConstraints(ImVec2(392.0f, 0.0f), ImVec2(392.0f, FLT_MAX));
    ImGui::SetNextWindowBgAlpha(0.94f);
    const std::string windowName = "###bahamut-addon-" + std::string(addonId);
    ImGuiWindowFlags  flags      = ImGuiWindowFlags_AlwaysAutoResize |
                                   ImGuiWindowFlags_NoCollapse |
                                   ImGuiWindowFlags_NoMove |
                                   ImGuiWindowFlags_NoTitleBar;
    if (locked || !ImGui::GetIO().KeyShift)
    {
        flags |= ImGuiWindowFlags_NoInputs;
    }
    ImGui::PushStyleColor(ImGuiCol_WindowBg, ImVec4(0.035f, 0.045f, 0.065f, 0.94f));
    if (ImGui::Begin(windowName.c_str(), nullptr, flags))
    {
        const bool  idle    = rowCount == 0 || rows == nullptr;
        const char* heading = std::string_view(mode) == "hps" ? "HPS" : "DPS";
        ImGui::TextColored(ImVec4(0.60f, 0.80f, 0.96f, 1.0f), "%s", heading);
        if (!idle)
        {
            ImGui::Separator();
            double maximumRate = 0.0;
            for (std::size_t index = 0; index < rowCount; ++index)
            {
                if (std::isfinite(rows[index].rate) && rows[index].rate > maximumRate)
                {
                    maximumRate = rows[index].rate;
                }
            }
            for (std::size_t index = 0; index < rowCount; ++index)
            {
                const AddonCombatMeterRow& row = rows[index];
                ImGui::PushID(static_cast<int>(index));
                char metric[128]{};
                char rate[32]{};
                if (row.rate >= 10000000.0)
                {
                    std::snprintf(rate, sizeof(rate), "%.1e", row.rate);
                }
                else
                {
                    std::snprintf(rate, sizeof(rate), "%.1f", row.rate);
                }
                const bool healingMode = std::string_view(mode) == "hps";
                if (healingMode)
                {
                    std::snprintf(metric, sizeof(metric), "%s HPS | %llu HEAL", rate, static_cast<unsigned long long>(row.amount));
                }
                else
                {
                    std::snprintf(metric, sizeof(metric), "%s DPS | %llu DMG | ACC %s", rate, static_cast<unsigned long long>(row.amount), row.accuracy.c_str());
                }
                const float metricWidth = ImGui::CalcTextSize(metric).x;
                const float nameWidth   = ImGui::CalcTextSize(row.name.c_str()).x;
                const float rowWidth    = ImGui::GetContentRegionAvail().x;
                ImGui::TextWrapped("%s", row.name.c_str());
                if (nameWidth + ImGui::GetStyle().ItemSpacing.x + metricWidth <= rowWidth)
                {
                    ImGui::SameLine();
                    ImGui::SetCursorPosX(std::max(ImGui::GetCursorPosX(),
                                                  ImGui::GetWindowContentRegionMax().x - metricWidth));
                    ImGui::TextColored(ImVec4(0.72f, 0.78f, 0.87f, 1.0f), "%s", metric);
                }
                else
                {
                    ImGui::TextColored(ImVec4(0.72f, 0.78f, 0.87f, 1.0f), "%s", metric);
                }

                const ImVec2 barOrigin = ImGui::GetCursorScreenPos();
                const float  barWidth  = ImGui::GetContentRegionAvail().x;
                const float  fillWidth = maximumRate > 0.0 && std::isfinite(row.rate)
                                             ? barWidth * static_cast<float>(std::clamp(row.rate / maximumRate, 0.0, 1.0))
                                             : 0.0f;
                ImDrawList*  draw      = ImGui::GetWindowDrawList();
                draw->AddRectFilled(barOrigin, ImVec2(barOrigin.x + barWidth, barOrigin.y + 6.0f), IM_COL32(42, 49, 62, 255), 3.0f);
                if (fillWidth > 0.0f)
                {
                    draw->AddRectFilled(barOrigin, ImVec2(barOrigin.x + fillWidth, barOrigin.y + 6.0f), ParseMeterColor(row.color), 3.0f);
                }
                ImGui::Dummy(ImVec2(barWidth, 6.0f));
                ImGui::Dummy(ImVec2(0.0f, 1.0f));
                ImGui::PopID();
            }
        }
        if (incomplete)
        {
            ImGui::TextColored(ImVec4(1.0f, 0.68f, 0.30f, 1.0f), "INCOMPLETE");
        }
    }
    const ImGuiIO& io = ImGui::GetIO();
    if (!locked && io.KeyShift && ImGui::IsWindowHovered(ImGuiHoveredFlags_AllowWhenBlockedByActiveItem) && ImGui::IsMouseClicked(ImGuiMouseButton_Left))
    {
        gDraggedAddonId = addonId;
    }
    if (locked && gDraggedAddonId == addonId)
    {
        gDraggedAddonId.clear();
    }
    else if (gDraggedAddonId == addonId)
    {
        if (!io.KeyShift || !ImGui::IsMouseDown(ImGuiMouseButton_Left))
        {
            gDraggedAddonId.clear();
            gSaveLayoutRequested = true;
        }
        else
        {
            const ImVec2 position = ImGui::GetWindowPos();
            ImGui::SetWindowPos(ImVec2(position.x + io.MouseDelta.x,
                                       position.y + io.MouseDelta.y));
        }
    }
    ImGui::End();
    ImGui::PopStyleColor();
}

void DrawZoneNamePopup(std::string_view text, float opacity)
{
    const auto             separator = text.find('\n');
    const std::string_view region    = separator == std::string_view::npos ? std::string_view{} : text.substr(0, separator);
    const std::string_view area      = separator == std::string_view::npos ? text : text.substr(separator + 1);
    const ImVec2           origin    = ImGui::GetWindowPos();
    const ImVec2           size      = ImGui::GetWindowSize();
    ImDrawList*            draw      = ImGui::GetWindowDrawList();
    const float            fade      = std::clamp(opacity, 0.0f, 1.0f);
    const auto             color     = [fade](int red, int green, int blue, int alpha)
    {
        return IM_COL32(red, green, blue, static_cast<int>(alpha * fade));
    };
    const float left      = origin.x;
    const float right     = origin.x + size.x;
    const float top       = origin.y;
    const float bottom    = origin.y + size.y;
    const float center    = left + size.x * 0.5f;
    const float fadeLeft  = left + size.x * 0.22f;
    const float fadeRight = left + size.x * 0.78f;
    draw->AddRectFilledMultiColor(ImVec2(left, top), ImVec2(fadeLeft, bottom), color(8, 10, 13, 0), color(8, 10, 13, 140), color(8, 10, 13, 140), color(8, 10, 13, 0));
    draw->AddRectFilledMultiColor(ImVec2(fadeLeft, top), ImVec2(center, bottom), color(8, 10, 13, 140), color(8, 10, 13, 176), color(8, 10, 13, 176), color(8, 10, 13, 140));
    draw->AddRectFilledMultiColor(ImVec2(center, top), ImVec2(fadeRight, bottom), color(8, 10, 13, 176), color(8, 10, 13, 140), color(8, 10, 13, 140), color(8, 10, 13, 176));
    draw->AddRectFilledMultiColor(ImVec2(fadeRight, top), ImVec2(right, bottom), color(8, 10, 13, 140), color(8, 10, 13, 0), color(8, 10, 13, 0), color(8, 10, 13, 140));

    const auto drawCentered = [&](std::string_view value, ImFont* font, float fontSize, float y, ImU32 foreground, float shadowDepth)
    {
        if (value.empty())
        {
            return;
        }
        font                  = font != nullptr ? font : ImGui::GetFont();
        const ImVec2 measured = font->CalcTextSizeA(fontSize, FLT_MAX, 0.0f, value.data(), value.data() + value.size());
        if (measured.x > size.x - 72.0f)
        {
            fontSize *= (size.x - 72.0f) / measured.x;
        }
        const ImVec2 textSize = font->CalcTextSizeA(fontSize, FLT_MAX, 0.0f, value.data(), value.data() + value.size());
        const ImVec2 position(left + (size.x - textSize.x) * 0.5f, top + y);
        draw->AddText(font, fontSize, ImVec2(position.x + shadowDepth, position.y + shadowDepth), color(0, 0, 0, 230), value.data(), value.data() + value.size());
        draw->AddText(font, fontSize, position, foreground, value.data(), value.data() + value.size());
    };
    drawCentered(region, gZoneRegionFont, 13.0f, 14.0f, color(231, 215, 173, 255), 1.0f);
    drawCentered(area, gZoneAreaFont, 30.0f, 37.0f, color(255, 247, 227, 255), 2.0f);

    const float ruleY     = top + 94.0f;
    const ImU32 ruleColor = color(216, 195, 143, 153);
    draw->AddLine(ImVec2(left + 36.0f, ruleY), ImVec2(center - 12.0f, ruleY), ruleColor);
    draw->AddLine(ImVec2(center + 12.0f, ruleY), ImVec2(right - 36.0f, ruleY), ruleColor);
    const ImVec2 diamond[] = {
        ImVec2(center, ruleY - 4.0f), ImVec2(center + 4.0f, ruleY), ImVec2(center, ruleY + 4.0f), ImVec2(center - 4.0f, ruleY)
    };
    draw->AddConvexPolyFilled(diamond, 4, color(216, 195, 143, 255));
}

void DrawAddonRawText(void* context, const char* addonId, const char* text, float red, float green, float blue, float alpha, bool locked, float fontSize)
{
    UNREFERENCED_PARAMETER(context);
    const ImGuiViewport*   viewport = ImGui::GetMainViewport();
    const std::string_view id(addonId);
    const bool             areaPopup = id == "zonename";
    if (areaPopup)
    {
        ImGui::SetNextWindowPos(ImVec2(
                                    viewport->WorkPos.x + viewport->WorkSize.x * 0.5f,
                                    viewport->WorkPos.y + 115.0f),
                                ImGuiCond_FirstUseEver,
                                ImVec2(0.5f, 0.0f));
        ImGui::SetNextWindowSize(ImVec2(std::min(620.0f, viewport->WorkSize.x), 106.0f));
    }
    else if (id == "distance")
    {
        ImGui::SetNextWindowPos(ImVec2(
                                    viewport->WorkPos.x + viewport->WorkSize.x - 28.0f,
                                    viewport->WorkPos.y + viewport->WorkSize.y * 0.5f),
                                ImGuiCond_FirstUseEver,
                                ImVec2(1.0f, 0.5f));
    }
    else if (id == "targethp")
    {
        ImGui::SetNextWindowPos(ImVec2(
                                    viewport->WorkPos.x + viewport->WorkSize.x * 0.25f,
                                    viewport->WorkPos.y + 16.0f),
                                ImGuiCond_FirstUseEver,
                                ImVec2(0.5f, 0.0f));
    }
    else
    {
        ImGui::SetNextWindowPos(ImVec2(
                                    viewport->WorkPos.x + viewport->WorkSize.x - 8.0f,
                                    viewport->WorkPos.y + 8.0f),
                                ImGuiCond_FirstUseEver,
                                ImVec2(1.0f, 0.0f));
    }
    const std::string windowName = id == "distance" || id == "targethp"
                                       ? "###bahamut-addon-" + std::string(addonId)
                                       : "###bahamut-addon-raw-" + std::string(addonId);
    ImGuiWindowFlags  flags      = ImGuiWindowFlags_NoDecoration | ImGuiWindowFlags_NoBackground | ImGuiWindowFlags_NoMove;
    if (!areaPopup)
    {
        flags |= ImGuiWindowFlags_AlwaysAutoResize;
    }
    if (locked || !ImGui::GetIO().KeyShift)
    {
        flags |= ImGuiWindowFlags_NoInputs;
    }
    if (ImGui::Begin(windowName.c_str(), nullptr, flags))
    {
        ImGui::SetWindowFontScale(fontSize > 0.0F ? fontSize / ImGui::GetFont()->FontSize : 1.0F);
        if (areaPopup)
        {
            DrawZoneNamePopup(text, alpha);
        }
        else
        {
            ImGui::TextColored(ImVec4(red, green, blue, alpha), "%s", text);
        }
    }
    const ImGuiIO& io = ImGui::GetIO();
    if (!locked && io.KeyShift && ImGui::IsWindowHovered(ImGuiHoveredFlags_AllowWhenBlockedByActiveItem) && ImGui::IsMouseClicked(ImGuiMouseButton_Left))
    {
        gDraggedAddonId = addonId;
    }
    if (locked && gDraggedAddonId == addonId)
    {
        gDraggedAddonId.clear();
    }
    else if (gDraggedAddonId == addonId)
    {
        if (!io.KeyShift || !ImGui::IsMouseDown(ImGuiMouseButton_Left))
        {
            gDraggedAddonId.clear();
            gSaveLayoutRequested = true;
        }
        else
        {
            const ImVec2 position = ImGui::GetWindowPos();
            ImGui::SetWindowPos(ImVec2(
                position.x + io.MouseDelta.x, position.y + io.MouseDelta.y));
        }
    }
    ImGui::End();
}

void DrawAddonTargetlines(void* context, const char* addonId)
{
    UNREFERENCED_PARAMETER(context);
    if (std::string_view(addonId) != "targetlines")
    {
        return;
    }
    const auto targetlines = bahamut_client::TargetlinesFrame();
    const auto displaySize = ImGui::GetIO().DisplaySize;
    // Projection uses back-buffer pixels; draw only at a matching display size.
    if (targetlines && displaySize.x == static_cast<float>(targetlines->backBufferWidth) &&
        displaySize.y == static_cast<float>(targetlines->backBufferHeight))
    {
        auto* draw = ImGui::GetBackgroundDrawList();
        for (std::size_t arcIndex = 0; arcIndex < targetlines->count; ++arcIndex)
        {
            const auto& arc   = targetlines->arcs[arcIndex];
            const int   red   = arc.friendly ? 42 : 255;
            const int   green = arc.friendly ? 255 : 48;
            const int   blue  = arc.friendly ? 112 : 72;
            const auto  color = [&](int alpha)
            {
                return IM_COL32(red, green, blue, static_cast<int>(alpha * arc.opacity));
            };
            // Layered strokes provide a soft beam without a texture asset.
            for (std::size_t pieceIndex = 0; pieceIndex < arc.pieces.count; ++pieceIndex)
            {
                const auto&  piece = arc.pieces.segments[pieceIndex];
                const ImVec2 source{ piece.source.x, piece.source.y };
                const ImVec2 target{ piece.target.x, piece.target.y };
                draw->AddLine(source, target, color(28), 7.0F);
                draw->AddLine(source, target, color(76), 4.0F);
                draw->AddLine(source, target, color(230), 1.8F);
                draw->AddLine(source, target, IM_COL32(255, 240, 238, static_cast<int>(170.0F * arc.opacity)), 0.65F);
            }
        }
    }
}

void RecordFillModeTestResult(IDirect3DDevice9* device)
{
    if (!IsWireframeEnabled() || InterlockedCompareExchange(&gFillModeTestRecorded, 1, 0) != 0)
    {
        return;
    }
    wchar_t     path[32768]{};
    const DWORD count = GetEnvironmentVariableW(
        L"BAHAMUT_TEST_OVERLAY_FILL_MODE_RESULT_FILE", path, ARRAYSIZE(path));
    if (count == 0 || count >= ARRAYSIZE(path))
    {
        return;
    }
    DWORD         fillMode = 0;
    const bool    solid    = SUCCEEDED(device->GetRenderState(D3DRS_FILLMODE, &fillMode)) && fillMode == D3DFILL_SOLID;
    std::ofstream file(path, std::ios::trunc);
    if (file)
    {
        file << "overlay_fill_mode=" << (solid ? "solid" : "other") << "\n";
    }
}

} // namespace

void ConfigureOverlayWindow(HWND window)
{
    gWindow = window;
}

void ConfigureOverlayLayout(const std::filesystem::path& path)
{
    OverlayLayoutPath() = path;
}

bool BindOverlayDevice(IDirect3DDevice9* device)
{
    if (device == nullptr)
    {
        return false;
    }
    if (gOverlayDevice == nullptr)
    {
        gOverlayDevice = device;
    }
    return gOverlayDevice == device;
}

void UnbindOverlayDevice(IDirect3DDevice9* device)
{
    if (gOverlayDevice == device)
    {
        gOverlayDevice = nullptr;
    }
}

bool IsOverlayDevice(IDirect3DDevice9* device)
{
    return device != nullptr && device == gOverlayDevice;
}

bool IsOverlayDrawing(IDirect3DDevice9* device)
{
    return IsOverlayDevice(device) && InterlockedCompareExchange(&gOverlayDrawing, 0, 0) != 0;
}

void DrawOverlay(IDirect3DDevice9*                   device,
                 BahamutRuntimeTelemetry*            telemetry,
                 AddonHost*                          addonHost,
                 NativePluginHost*                   discordPluginHost,
                 bahamut_client::PlayerStateService* playerState)
{
    const HRESULT cooperativeLevel = device->TestCooperativeLevel();
    if (cooperativeLevel == D3DERR_DEVICELOST || cooperativeLevel == D3DERR_DEVICENOTRESET)
    {
        return;
    }
    if (!gInitialized && !InitializeOverlay(device))
    {
        ++telemetry->overlayFailureCount;
        return;
    }
    if (!gBackendReady && !CompleteOverlayReset())
    {
        ++telemetry->overlayFailureCount;
        return;
    }
    UpdateControllerObservation();

    if (discordPluginHost != nullptr && playerState != nullptr)
    {
        BahamutNativePluginPlayerStateV1 state{};
        state.structSize    = sizeof(state);
        const auto snapshot = playerState->Snapshot();
        if (snapshot.has_value())
        {
            state.actorId     = snapshot->actorId;
            state.zoneId      = snapshot->zoneId;
            state.baseClassId = snapshot->baseClassId;
            state.jobId       = snapshot->jobId;
            state.level       = snapshot->level;
            state.flags       = BahamutNativePluginPlayerStateHasCharacter;
            if (!snapshot->areaName.empty())
            {
                state.flags |= BahamutNativePluginPlayerStateHasArea;
            }
            if ((snapshot->baseClassId != 0u || snapshot->jobId != 0u) &&
                snapshot->level != 0u)
            {
                state.flags |= BahamutNativePluginPlayerStateHasClassJob;
            }
            const auto copyText = [](char*              destination,
                                     std::size_t        capacity,
                                     const std::string& source)
            {
                const std::size_t count = std::min(capacity - 1u, source.size());
                std::memcpy(destination, source.data(), count);
                destination[count] = '\0';
            };
            copyText(state.displayName, sizeof(state.displayName), snapshot->displayName);
            copyText(state.areaName, sizeof(state.areaName), snapshot->areaName);
        }
        static_cast<void>(discordPluginHost->PublishPlayerState(state));
    }

    LARGE_INTEGER now{};
    LARGE_INTEGER frequency{};
    QueryPerformanceCounter(&now);
    QueryPerformanceFrequency(&frequency);
    const double frameDelta = gLastAddonFrame.QuadPart == 0 || frequency.QuadPart == 0
                                  ? 0.0
                                  : static_cast<double>(now.QuadPart - gLastAddonFrame.QuadPart) / static_cast<double>(frequency.QuadPart);
    gLastAddonFrame         = now;
    if (addonHost != nullptr)
    {
        addonHost->Update(frameDelta);
    }
    if (!IsOverlayVisible())
    {
        return;
    }

    BahamutRuntimeTimingSnapshot timing{};
    if (!BahamutReadTimingSnapshot(*telemetry, timing))
    {
        ++telemetry->overlayFailureCount;
        return;
    }

    LARGE_INTEGER start{};
    QueryPerformanceCounter(&start);
    CompleteDeviceState state(device);
    if (!state.IsCaptured() || !state.BindBackBuffer())
    {
        ++telemetry->overlayFailureCount;
        return;
    }
    RecordBackBufferTestResult(state);
    ImGui_ImplDX9_NewFrame();
    ImGui_ImplWin32_NewFrame();
    ImGui::NewFrame();
    if (addonHost != nullptr)
    {
        gNextAddonWindowY = ImGui::GetMainViewport()->WorkPos.y + 16.0f;
        addonHost->Draw({ nullptr, &DrawAddonWindow, &DrawAddonRawText, &DrawAddonCombatMeter, &DrawAddonTargetlines });
    }
    InterlockedExchange(&gOverlayDrawing, 1);
    device->SetRenderState(D3DRS_FILLMODE, D3DFILL_SOLID);
    ImGui::Render();
    ImGui_ImplDX9_RenderDrawData(ImGui::GetDrawData());
    if (gSaveLayoutRequested || ImGui::GetIO().WantSaveIniSettings)
    {
        SaveOverlayLayout();
        gSaveLayoutRequested               = false;
        ImGui::GetIO().WantSaveIniSettings = false;
    }
    RecordFillModeTestResult(device);
    InterlockedExchange(&gOverlayDrawing, 0);

    const bool    restored = state.Restore();
    LARGE_INTEGER end{};
    QueryPerformanceCounter(&end);
    const LONGLONG elapsed = end.QuadPart - start.QuadPart;
    BahamutBeginTimingWrite(telemetry);
    telemetry->overlayTotalCounter.QuadPart += elapsed;
    telemetry->overlayMaxCounter.QuadPart = std::max(
        timing.overlayMaxCounter.QuadPart, elapsed);
    ++telemetry->overlayFrameCount;
    BahamutEndTimingWrite(telemetry);
    if (!restored)
    {
        ++telemetry->overlayFailureCount;
    }
}

void PrepareOverlayReset()
{
    gLastAddonFrame = {};
    if (gInitialized)
    {
        gBackendReady = false;
        ImGui_ImplDX9_InvalidateDeviceObjects();
    }
}

bool CompleteOverlayReset()
{
    if (!gInitialized)
    {
        return true;
    }
    if (HasTestFault(L"BAHAMUT_TEST_OVERLAY_BACKEND_RETRY"))
    {
        const LONG attempt = ++gBackendRetryAttempt;
        if (attempt == 1)
        {
            gBackendReady = false;
            RecordTestResult("backend_retry=first_failed\n");
            return false;
        }
    }
    gBackendReady = ImGui_ImplDX9_CreateDeviceObjects();
    if (gBackendReady && HasTestFault(L"BAHAMUT_TEST_OVERLAY_BACKEND_RETRY") && gBackendRetryAttempt >= 2)
    {
        RecordTestResult("backend_retry=ok\n");
    }
    return gBackendReady;
}
