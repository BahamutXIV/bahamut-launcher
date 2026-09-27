local name             = nil
local region           = ''
local elapsed          = 0
local visible_seconds  = 6
local fade_in_seconds  = 0.4
local fade_out_seconds = 1.4

function area_changed(zone_id, area_name, region_name)
    if zone_id == 0 or area_name == '' then
        name    = nil
        region  = ''
        elapsed = 0
        return
    end

    name    = area_name
    region  = region_name or ''
    elapsed = 0
end

function update(delta_seconds)
    if name ~= nil and delta_seconds > 0 then
        elapsed = math.min(visible_seconds, elapsed + delta_seconds)
    end
end

function draw()
    if name == nil or elapsed >= visible_seconds then
        return
    end

    local alpha = math.min(1, elapsed / fade_in_seconds,
        (visible_seconds - elapsed) / fade_out_seconds)
    local text  = name
    if region ~= '' then
        text = string.upper(region) .. '\n' .. name
    end

    bahamut.raw_text(text, 1, 1, 1, alpha, false)
end
