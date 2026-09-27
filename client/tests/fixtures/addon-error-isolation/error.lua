function load()
    if io ~= nil or os ~= nil or package ~= nil or require ~= nil or debug ~= nil then
        error('forbidden Lua library is available')
    end

    local allowed =
    {
        settings_get = true,
        settings_set = true,
        player_state = true,
        window = true,
        raw_text = true,
        clipboard_set = true,
        chat_print = true,
    }
    for key in pairs(bahamut) do
        if not allowed[key] then
            error('unexpected host capability: ' .. key)
        end
    end

    if bahamut.settings_get('visible', 'missing') ~= 'missing' then
        error('settings leaked between addons')
    end

    bahamut.settings_set('scope', 'error-isolation-test')
    shared_probe = 'error-isolation-test'
end

function update(delta)
    if delta ~= 0.02 then
        error('unexpected frame delta')
    end

    error('intentional update failure')
end
