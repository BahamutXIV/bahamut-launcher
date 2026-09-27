local locked           = false
local red, green, blue = 1.0, 1.0, 1.0
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
    locked = bahamut.settings_get('locked', 'false') == 'true'
    if not set_color(bahamut.settings_get('color', '#FFFFFF')) then set_color('#FFFFFF') end

    if not set_size(bahamut.settings_get('size', '13')) then font_size = 13 end
end

function command(name)
    if name == '/targethp help' then
        bahamut.chat_print('/targethp lock')
        bahamut.chat_print('/targethp color #RRGGBB')
        bahamut.chat_print('/targethp size 8-48')
        return true
    end

    if name == '/targethp lock' then
        locked = not locked
        bahamut.settings_set('locked', locked and 'true' or 'false')
        bahamut.chat_print(locked and 'Target HP overlay locked' or 'Target HP overlay unlocked')
        return true
    end

    local color = name:match('^/targethp color (.+)$')
    if color then
        if set_color(color) then
            bahamut.settings_set('color', color:upper())
        else
            bahamut.chat_print('Usage: /targethp color #RRGGBB')
        end

        return true
    end

    local size = name:match('^/targethp size (.+)$')
    if size then
        if set_size(size) then
            bahamut.settings_set('size', tostring(font_size))
        else
            bahamut.chat_print('Usage: /targethp size 8-48')
        end

        return true
    end

    return false
end

function draw()
    local current, maximum, _, actor_id = bahamut.target_hp()
    if actor_id == nil or current == nil or maximum == nil then
        return
    end

    local percent = math.floor(current * 100 / maximum + 0.5)
    bahamut.raw_text(string.format('HP %d/%d (%d%%)', current, maximum, percent), red, green, blue, 1.0, locked, font_size)
end
