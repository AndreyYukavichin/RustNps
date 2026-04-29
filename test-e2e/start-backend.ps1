param(
    [int]$Port = 28081
)

$listener = [System.Net.HttpListener]::new()
$listener.Prefixes.Add("http://127.0.0.1:$Port/")
try {
    $listener.Start()
}
catch {
    Write-Error "BACKEND_START_FAILED $Port $($_.Exception.Message)"
    $listener.Close()
    exit 1
}

Write-Host "BACKEND_READY $Port"

try {
    while ($listener.IsListening) {
        $context = $listener.GetContext()
        $path = $context.Request.Url.AbsolutePath
        Write-Host ("BACKEND_REQUEST " + $context.Request.HttpMethod + " " + $path)
        if ($path -match '^/bytes/(\d+)$') {
            $size = [int]$Matches[1]
            $buffer = New-Object byte[] $size
            for ($i = 0; $i -lt $size; $i++) { $buffer[$i] = 65 }
            $context.Response.StatusCode = 200
            $context.Response.ContentType = 'application/octet-stream'
            $context.Response.ContentLength64 = $buffer.Length
            $context.Response.OutputStream.Write($buffer, 0, $buffer.Length)
            $context.Response.OutputStream.Close()
            continue
        }

        $body = [System.Text.Encoding]::UTF8.GetBytes("ok")
        $context.Response.StatusCode = 200
        $context.Response.ContentType = 'text/plain; charset=utf-8'
        $context.Response.ContentLength64 = $body.Length
        $context.Response.OutputStream.Write($body, 0, $body.Length)
        $context.Response.OutputStream.Close()
    }
}
finally {
    if ($listener.IsListening) {
        $listener.Stop()
    }
    $listener.Close()
}
