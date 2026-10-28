function load()
    local text   = bahamut.settings_get('slot', '1')
    local digits = text:match('^%s*(%d+)%s*$')
    -- Invalid settings reach the service as slot 0 and produce needs-user.
    bahamut.autologin_start(digits and tonumber(digits) or 0)
end
