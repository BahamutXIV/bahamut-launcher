local elapsed          = 0
local frames           = 0
local rate             = 0
local visible          = true
local locked           = false
local red, green, blue = 1.0, 0.0, 0.0
local font_size        = 13

local function set_color(value)
    local hex = value:match('^#?(%x%x%x%x%x%x)$')
    if not hex then return false end

    red   = tonumber(hex:sub(1, 2), 16) / 255
    green = tonumber(hex:sub(3, 4), 16) / 255
    blue  = tonumber(hex:sub(5, 6), 16) / 255
    return true
end

local function set_size(value)
    local size = tonumber(value)
    if not size or size < 8 or size > 48 or size ~= math.floor(size) then return false end

    font_size = size
    return true
end

function load()
    if shared_probe ~= nil or io ~= nil or os ~= nil or package ~= nil or debug ~= nil then
        error('addon state is not isolated')
    end

    visible = bahamut.settings_get('visible', 'true') ~= 'false'
    locked  = bahamut.settings_get('locked', 'false') == 'true'
    if not set_color(bahamut.settings_get('color', '#FF0000')) then set_color('#FF0000') end

    if not set_size(bahamut.settings_get('size', '13')) then font_size = 13 end

    bahamut.settings_set('visible', visible and 'true' or 'false')
end
function command(name)
    if name == '/fps help' then
        bahamut.chat_print('/fps')
        bahamut.chat_print('/fps lock')
        bahamut.chat_print('/fps color #RRGGBB')
        bahamut.chat_print('/fps size 8-48')
        return true
    end

    if name == '/fps' then
        visible = not visible
        bahamut.settings_set('visible', visible and 'true' or 'false')
        return true
    end

    if name == '/fps lock' then
        locked = not locked
        bahamut.settings_set('locked', locked and 'true' or 'false')
        bahamut.chat_print(locked and 'FPS overlay locked' or 'FPS overlay unlocked')
        return true
    end

    local color = name:match('^/fps color (.+)$')
    if color then
        if set_color(color) then
            bahamut.settings_set('color', color:upper())
        else
            bahamut.chat_print('Usage: /fps color #RRGGBB')
        end

        return true
    end

    local size = name:match('^/fps size (.+)$')
    if size then
        if set_size(size) then
            bahamut.settings_set('size', tostring(font_size))
        else
            bahamut.chat_print('Usage: /fps size 8-48')
        end

        return true
    end

    return false
end

function update(delta)
    elapsed = elapsed + delta
    frames  = frames + 1
    if elapsed >= 0.5 then
        rate    = frames / elapsed
        elapsed = 0
        frames  = 0
    end
end

function draw()
    if visible then
        bahamut.raw_text(string.format('%.0f', rate), red, green, blue, 1.0, locked, font_size)
    end
end
