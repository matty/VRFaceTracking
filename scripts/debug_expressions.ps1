param(
    [Parameter(Mandatory = $true)]
    [string]$Port
)

$BaseUrl = "http://localhost:$Port/debug/params"

function Set-Expression {
    param(
        [string]$Name,
        [hashtable]$Params
    )

    Write-Host "Setting Expression: $Name" -ForegroundColor Cyan
    
    $JsonPayload = $Params | ConvertTo-Json -Compress
    
    try {
        $response = Invoke-RestMethod -Method Post -Uri $BaseUrl -Body $JsonPayload -ContentType "application/json" -ErrorAction Stop
        Write-Host "  Success: $($response.status)" -ForegroundColor Green
    }
    catch {
        Write-Host "  Failed: $_" -ForegroundColor Red
    }
}

function Reset-Face {
    Write-Host "Resetting to Neutral..." -ForegroundColor Yellow
    $ResetParams = @{
        "JawOpen"          = 0.0
        "MouthSmileLeft"   = 0.0
        "MouthSmileRight"  = 0.0
        "EyeSquintLeft"    = 0.0
        "EyeSquintRight"   = 0.0
        "MouthFunnel"      = 0.0
        "MouthPucker"      = 0.0
        "BrowInnerUpLeft"  = 0.0
        "BrowInnerUpRight" = 0.0
        "CheekPuffLeft"    = 0.0
        "CheekPuffRight"   = 0.0
        "TongueOut"        = 0.0
        "MouthFrownLeft"   = 0.0
        "MouthFrownRight"  = 0.0
        "BrowPinchLeft"    = 0.0
        "BrowPinchRight"   = 0.0
        "EyeLeftOpenness"  = 1.0
        "EyeRightOpenness" = 1.0
    }
    Set-Expression -Name "Reset" -Params $ResetParams
}

# Jaw Open
Reset-Face
Start-Sleep -Seconds 1
Set-Expression -Name "Jaw Open" -Params @{ "JawOpen" = 1.0 }
Start-Sleep -Seconds 2

# Smile
Reset-Face
Start-Sleep -Seconds 1
Set-Expression -Name "Smile" -Params @{ 
    "MouthSmileLeft"  = 1.0
    "MouthSmileRight" = 1.0
    "EyeSquintLeft"   = 0.6
    "EyeSquintRight"  = 0.6
}
Start-Sleep -Seconds 2

# Surprise (Brows Up + Jaw Drop)
Reset-Face
Start-Sleep -Seconds 1
Set-Expression -Name "Surprise" -Params @{ 
    "BrowInnerUpLeft"  = 1.0
    "BrowInnerUpRight" = 1.0
    "JawOpen"          = 0.5
}
Start-Sleep -Seconds 2

# Pog (Funnel + Jaw)
Reset-Face
Start-Sleep -Seconds 1
Set-Expression -Name "Pog" -Params @{ 
    "MouthFunnel" = 1.0
    "JawOpen"     = 0.6
}
Start-Sleep -Seconds 2

# Kiss (Pucker)
Reset-Face
Start-Sleep -Seconds 1
Set-Expression -Name "Kiss" -Params @{ 
    "MouthPucker" = 1.0
}
Start-Sleep -Seconds 2

# Cheek Puff
Reset-Face
Start-Sleep -Seconds 1
Set-Expression -Name "Cheek Puff" -Params @{ 
    "CheekPuffLeft"  = 1.0
    "CheekPuffRight" = 1.0
}
Start-Sleep -Seconds 2

# Tongue Out
Reset-Face
Start-Sleep -Seconds 1
Set-Expression -Name "Tongue Out" -Params @{ 
    "TongueOut" = 1.0
}
Start-Sleep -Seconds 2

# Sad (Frown)
Reset-Face
Start-Sleep -Seconds 1
Set-Expression -Name "Sad" -Params @{ 
    "MouthFrownLeft"  = 1.0
    "MouthFrownRight" = 1.0
    "BrowPinchLeft"   = 0.8
    "BrowPinchRight"  = 0.8
}
Start-Sleep -Seconds 2

# End
Reset-Face
Write-Host "Test Sequence Complete!" -ForegroundColor Green
