function packet(direction, captured_at_ms, size, frame_hex)
    local row = table.concat(
        {
            string.format('%.0f', captured_at_ms), direction, tostring(size), frame_hex,
        }, ',')
    if not bahamut.packetlog_write(row) then
        error('packet log write failed')
    end
end

function command(name)
    if name == '/packetlogger start' then
        if bahamut.packetlog_start() then
            bahamut.chat_print('Packet capture recording: ' .. bahamut.packetlog_status().file)
        else
            bahamut.chat_print('Packet capture could not start')
        end

        return true
    end

    if name == '/packetlogger stop' then
        bahamut.chat_print(bahamut.packetlog_stop() and
            'Packet capture stopped' or 'Packet capture already stopped')
        return true
    end

    if name == '/packetlogger' or name == '/packetlogger status' then
        local status = bahamut.packetlog_status()
        bahamut.chat_print(string.format(
            'Packet capture %s | captured %d, written %d, dropped %d, queued %d, peak %d | %s',
            status.recording and 'recording' or 'stopped',
            status.captured, status.written, status.dropped,
            status.queued, status.queue_high_water, status.file))
        return true
    end

    return false
end
