<#
.SYNOPSIS
    One-time setup so GitHub Actions can sign releases with Azure Artifact Signing.

.DESCRIPTION
    Run once, signed in to Azure CLI (`az login`) as an owner of the subscription
    that holds the signing account, and to GitHub CLI (`gh auth login`) as an
    admin of the repository. Idempotent: re-running reuses what already exists.

    Creates / ensures:
      1. An Entra app registration + service principal for GitHub Actions.
      2. A federated credential trusting GitHub's OIDC token for
         repo:<Repo>:environment:<Environment>  (no client secret anywhere).
      3. The "Artifact Signing Certificate Profile Signer" role for that service
         principal on the signing account (data-plane role; Owner is not enough).
      4. The GitHub environment and its AZURE_CLIENT_ID / AZURE_TENANT_ID /
         AZURE_SUBSCRIPTION_ID secrets (IDs only; nothing sensitive).

    -GrantCurrentUser additionally gives your own login the Signer role so
    tools\sign-release.ps1 works locally.

.EXAMPLE
    .\tools\setup-ci-signing.ps1 -GrantCurrentUser
#>
[CmdletBinding()]
param(
    [string]$Repo = 'LorexValkin/OBR-Music-Tool',
    [string]$Environment = 'release',
    [string]$AppName = 'obr-music-tool-github-release',
    [string]$SigningAccount = 'computerworksrmmsign',
    [switch]$GrantCurrentUser
)

$ErrorActionPreference = 'Stop'
$SignerRole = 'Artifact Signing Certificate Profile Signer'

function Invoke-Az {
    $out = & az @args 2>&1
    if ($LASTEXITCODE -ne 0) { throw "az $($args -join ' ')`n$out" }
    return $out
}

# ---- 0. Context -------------------------------------------------------------
$subscriptionId = (Invoke-Az account show --query id -o tsv).Trim()
$tenantId       = (Invoke-Az account show --query tenantId -o tsv).Trim()
$accountId      = (Invoke-Az resource list --resource-type Microsoft.CodeSigning/codeSigningAccounts `
                       --query "[?name=='$SigningAccount'].id | [0]" -o tsv).Trim()
if (-not $accountId) { throw "Signing account '$SigningAccount' not found in subscription $subscriptionId." }
Write-Host "Subscription : $subscriptionId"
Write-Host "Tenant       : $tenantId"
Write-Host "Signing acct : $accountId"

# ---- 1. App registration + service principal --------------------------------
$appId = (Invoke-Az ad app list --display-name $AppName --query '[0].appId' -o tsv).Trim()
if (-not $appId) {
    Write-Host "Creating app registration '$AppName' ..."
    $appId = (Invoke-Az ad app create --display-name $AppName --sign-in-audience AzureADMyOrg --query appId -o tsv).Trim()
}
Write-Host "App (client) : $appId"

$spObjectId = (Invoke-Az ad sp list --filter "appId eq '$appId'" --query '[0].id' -o tsv).Trim()
if (-not $spObjectId) {
    Write-Host 'Creating service principal ...'
    $spObjectId = (Invoke-Az ad sp create --id $appId --query id -o tsv).Trim()
}
Write-Host "SP object id : $spObjectId"

# ---- 2. Federated credential (GitHub OIDC) ----------------------------------
$subject = "repo:${Repo}:environment:${Environment}"
$existing = (Invoke-Az ad app federated-credential list --id $appId --query "[?subject=='$subject'].name | [0]" -o tsv).Trim()
if (-not $existing) {
    Write-Host "Creating federated credential for $subject ..."
    $params = @{
        name        = 'github-release-environment'
        issuer      = 'https://token.actions.githubusercontent.com'
        subject     = $subject
        audiences   = @('api://AzureADTokenExchange')
        description = "GitHub Actions, $Repo, environment '$Environment'"
    } | ConvertTo-Json -Compress
    $paramFile = Join-Path $env:TEMP 'obr-fic.json'
    Set-Content -Path $paramFile -Value $params -Encoding ascii
    Invoke-Az ad app federated-credential create --id $appId --parameters $paramFile | Out-Null
    Remove-Item $paramFile -Force
} else {
    Write-Host "Federated credential already present ($existing)."
}

# ---- 3. Signer role on the signing account ----------------------------------
Write-Host "Granting '$SignerRole' to the service principal ..."
Invoke-Az role assignment create --assignee-object-id $spObjectId --assignee-principal-type ServicePrincipal `
    --role $SignerRole --scope $accountId | Out-Null

if ($GrantCurrentUser) {
    $me = (Invoke-Az ad signed-in-user show --query id -o tsv).Trim()
    Write-Host "Granting '$SignerRole' to the signed-in user ($me) ..."
    Invoke-Az role assignment create --assignee-object-id $me --assignee-principal-type User `
        --role $SignerRole --scope $accountId | Out-Null
}

# ---- 4. GitHub environment + secrets ----------------------------------------
Write-Host "Ensuring GitHub environment '$Environment' on $Repo ..."
gh api -X PUT "repos/$Repo/environments/$Environment" | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'gh api failed (are you logged in with repo admin rights?)' }

foreach ($pair in @(
        @{ name = 'AZURE_CLIENT_ID';       value = $appId },
        @{ name = 'AZURE_TENANT_ID';       value = $tenantId },
        @{ name = 'AZURE_SUBSCRIPTION_ID'; value = $subscriptionId })) {
    gh secret set $pair.name --repo $Repo --env $Environment --body $pair.value
    if ($LASTEXITCODE -ne 0) { throw "gh secret set $($pair.name) failed" }
}

Write-Host ''
Write-Host 'Done. Role assignments can take a few minutes to propagate.'
Write-Host "Optional: add required reviewers to the '$Environment' environment in GitHub so a human approves each signing run."
Write-Host 'Release: bump version in Cargo.toml, commit, then  git tag vX.Y.Z && git push --tags'
