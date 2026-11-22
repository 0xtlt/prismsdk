# Windows PowerShell, for `prism build windows` signing with signtool: the
# developer's key in a key container of this user's key store, which signtool
# signs with (`/csp`, `/kc`), so no key file and no password reaches signtool.
#
# PRISM_KEY_ACTION=import: opens the .pfx in PRISM_PFX_IN with the password read
#   from standard input (UTF-8), its key held in memory only (EphemeralKeySet),
#   and imports that key, non-exportable, into a new container named
#   PRISM_KEY_NAME: in the CNG "Microsoft Software Key Storage Provider" (RSA
#   and ECDSA), else, for RSA, in the "Microsoft Enhanced RSA and AES
#   Cryptographic Provider". Writes the public certificate to PRISM_CER_OUT and
#   prints the provider's name. No certificate store is touched. First deletes
#   the containers a crashed run left: named prism-sign-<pid>-<hex>, their
#   process gone.
# PRISM_KEY_ACTION=delete: deletes the container PRISM_KEY_NAME, wherever it is,
#   and checks it is gone.
#
# Only containers named prism-sign-<pid>-<hex> are ever deleted.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
try {
    Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text.RegularExpressions;
public static class PrismSignKeys {
    public const string Ksp = "Microsoft Software Key Storage Provider";
    public const string Csp = "Microsoft Enhanced RSA and AES Cryptographic Provider";
    const int Silent = 0x40, Persist = unchecked((int)0x80000000), RsaAes = 24;
    const uint VerifyContext = 0xF0000000, DeleteKeySet = 0x10, CspSilent = 0x40;
    [StructLayout(LayoutKind.Sequential)] struct NKeyName { public IntPtr Name; public IntPtr Algid; public int KeySpec; public int Flags; }
    [DllImport("ncrypt.dll", CharSet = CharSet.Unicode)] static extern int NCryptOpenStorageProvider(out IntPtr provider, string name, int flags);
    [DllImport("ncrypt.dll", CharSet = CharSet.Unicode)] static extern int NCryptCreatePersistedKey(IntPtr provider, out IntPtr key, string algorithm, string name, int keySpec, int flags);
    [DllImport("ncrypt.dll", CharSet = CharSet.Unicode)] static extern int NCryptOpenKey(IntPtr provider, out IntPtr key, string name, int keySpec, int flags);
    [DllImport("ncrypt.dll", CharSet = CharSet.Unicode)] static extern int NCryptSetProperty(IntPtr obj, string property, byte[] value, int size, int flags);
    [DllImport("ncrypt.dll", CharSet = CharSet.Unicode)] static extern int NCryptEnumKeys(IntPtr provider, string scope, out IntPtr keyName, ref IntPtr state, int flags);
    [DllImport("ncrypt.dll")] static extern int NCryptFinalizeKey(IntPtr key, int flags);
    [DllImport("ncrypt.dll")] static extern int NCryptDeleteKey(IntPtr key, int flags);
    [DllImport("ncrypt.dll")] static extern int NCryptFreeObject(IntPtr obj);
    [DllImport("ncrypt.dll")] static extern int NCryptFreeBuffer(IntPtr buffer);
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)] static extern bool CryptAcquireContext(out IntPtr prov, string container, string provider, int type, uint flags);
    [DllImport("advapi32.dll", SetLastError = true)] static extern bool CryptGetProvParam(IntPtr prov, int param, byte[] data, ref int size, int flags);
    [DllImport("advapi32.dll")] static extern bool CryptReleaseContext(IntPtr prov, int flags);

    static void Check(int status) { if (status != 0) throw new Win32Exception(status); }

    // Only Prism's own signing containers may be deleted.
    public static bool Ours(string name) { return Regex.IsMatch(name, "^prism-sign-[0-9]+-[0-9a-f]+$"); }

    // A private key blob (RSAFULLPRIVATEBLOB, ECCPRIVATEBLOB) into a new,
    // persisted, non-exportable CNG key named `name`. signtool finds an ECDSA
    // key only with the legacy key spec AT_SIGNATURE (2).
    public static void Import(string name, string algorithm, int keySpec, string blobType, byte[] blob) {
        IntPtr prov, key = IntPtr.Zero;
        Check(NCryptOpenStorageProvider(out prov, Ksp, 0));
        try {
            // Fails if a key of that name exists: none is overwritten.
            Check(NCryptCreatePersistedKey(prov, out key, algorithm, name, keySpec, 0));
            try {
                Check(NCryptSetProperty(key, blobType, blob, blob.Length, Silent));
                Check(NCryptSetProperty(key, "Export Policy", BitConverter.GetBytes(0), 4, Persist | Silent));
                Check(NCryptFinalizeKey(key, Silent));
            } catch {
                NCryptDeleteKey(key, Silent);
                key = IntPtr.Zero;
                throw;
            }
        } finally {
            if (key != IntPtr.Zero) NCryptFreeObject(key);
            NCryptFreeObject(prov);
        }
    }

    // Deletes Prism's container `name`, in either provider, with any key spec.
    public static void Delete(string name) {
        if (!Ours(name)) throw new ArgumentException(name + " is not a Prism signing key");
        IntPtr handle;
        // CryptoAPI first: the CNG provider also sees its containers, and
        // cannot delete them.
        CryptAcquireContext(out handle, name, Csp, RsaAes, DeleteKeySet | CspSilent);
        IntPtr prov;
        if (NCryptOpenStorageProvider(out prov, Ksp, 0) != 0) return;
        try {
            foreach (int spec in new[] { 0, 2, 1 }) {
                IntPtr key;
                if (NCryptOpenKey(prov, out key, name, spec, Silent) == 0) NCryptDeleteKey(key, Silent);
            }
        } finally {
            NCryptFreeObject(prov);
        }
    }

    // Whether a container `name` exists, in either provider.
    public static bool Exists(string name) {
        IntPtr handle;
        if (CryptAcquireContext(out handle, name, Csp, RsaAes, CspSilent)) {
            CryptReleaseContext(handle, 0);
            return true;
        }
        IntPtr prov;
        if (NCryptOpenStorageProvider(out prov, Ksp, 0) != 0) return false;
        try {
            foreach (int spec in new[] { 0, 2, 1 }) {
                IntPtr key;
                if (NCryptOpenKey(prov, out key, name, spec, Silent) == 0) {
                    NCryptFreeObject(key);
                    return true;
                }
            }
            return false;
        } finally {
            NCryptFreeObject(prov);
        }
    }

    // The names of this user's containers in both providers.
    public static List<string> Names() {
        List<string> names = new List<string>();
        IntPtr prov, state = IntPtr.Zero, item;
        if (NCryptOpenStorageProvider(out prov, Ksp, 0) == 0) {
            try {
                while (NCryptEnumKeys(prov, null, out item, ref state, Silent) == 0) {
                    NKeyName k = (NKeyName)Marshal.PtrToStructure(item, typeof(NKeyName));
                    names.Add(Marshal.PtrToStringUni(k.Name));
                    NCryptFreeBuffer(item);
                }
            } finally {
                if (state != IntPtr.Zero) NCryptFreeBuffer(state);
                NCryptFreeObject(prov);
            }
        }
        if (CryptAcquireContext(out prov, null, Csp, RsaAes, VerifyContext | CspSilent)) {
            try {
                byte[] data = new byte[1024];
                int flags = 1; // CRYPT_FIRST
                while (true) {
                    int size = data.Length;
                    if (!CryptGetProvParam(prov, 2, data, ref size, flags)) break; // PP_ENUMCONTAINERS
                    int end = Array.IndexOf(data, (byte)0, 0, size);
                    string name = System.Text.Encoding.ASCII.GetString(data, 0, end < 0 ? size : end);
                    if (!names.Contains(name)) names.Add(name);
                    flags = 0;
                }
            } finally {
                CryptReleaseContext(prov, 0);
            }
        }
        return names;
    }
}
'@

    $name = $env:PRISM_KEY_NAME
    if (-not [PrismSignKeys]::Ours($name)) { throw "$name is not a Prism signing key name" }
    if ($env:PRISM_KEY_ACTION -eq 'delete') {
        [PrismSignKeys]::Delete($name)
        if ([PrismSignKeys]::Exists($name)) { throw "the key container $name is still there" }
        exit 0
    }

    # Containers a crashed run left: ours only, once their process is gone
    # (best effort: a failure here does not stop signing).
    foreach ($n in [PrismSignKeys]::Names()) {
        if ($n -match '^prism-sign-(\d+)-[0-9a-f]+$' -and -not (Get-Process -Id $Matches[1] -ErrorAction SilentlyContinue)) {
            try { [PrismSignKeys]::Delete($n) } catch {}
        }
    }

    $in = [Console]::OpenStandardInput()
    $buffer = New-Object IO.MemoryStream
    $in.CopyTo($buffer)
    $password = [Text.Encoding]::UTF8.GetString($buffer.ToArray())
    $flags = [Security.Cryptography.X509Certificates.X509KeyStorageFlags]'EphemeralKeySet'
    $certs = New-Object Security.Cryptography.X509Certificates.X509Certificate2Collection
    $certs.Import([IO.File]::ReadAllBytes($env:PRISM_PFX_IN), $password, $flags)
    $cert = $certs | Where-Object HasPrivateKey | Select-Object -First 1
    if (-not $cert) { throw 'the .pfx holds no private key' }
    [IO.File]::WriteAllBytes($env:PRISM_CER_OUT, $cert.RawData)

    $rsa = [Security.Cryptography.X509Certificates.RSACertificateExtensions]::GetRSAPrivateKey($cert)
    $key = if ($rsa) { $rsa } else { [Security.Cryptography.X509Certificates.ECDsaCertificateExtensions]::GetECDsaPrivateKey($cert) }
    if (-not $key) { throw 'the .pfx key is neither RSA nor ECDSA' }
    $provider = $null
    $why = ''
    if ($key.PSObject.Properties['Key'] -and $key.Key -is [Security.Cryptography.CngKey]) {
        # This ephemeral key lives in this process only: its export policy may change.
        $policy = [BitConverter]::GetBytes([int]3) # AllowExport, AllowPlaintextExport
        $key.Key.SetProperty((New-Object Security.Cryptography.CngProperty 'Export Policy', $policy, 'None'))
        if ($rsa) {
            $algorithm, $spec, $blobType = 'RSA', 0, 'RSAFULLPRIVATEBLOB'
        } else {
            # A .pfx's EC key loads as ECDH_P256 (P384, P521), which signtool
            # does not sign with: the same key as ECDSA (its blob's magic
            # ECK<n> becomes ECS<n>).
            $algorithm, $spec, $blobType = ($key.Key.Algorithm.Algorithm -replace '^ECDH_', 'ECDSA_'), 2, 'ECCPRIVATEBLOB'
        }
        $blob = $key.Key.Export((New-Object Security.Cryptography.CngKeyBlobFormat $blobType))
        if ($blobType -eq 'ECCPRIVATEBLOB' -and $blob[2] -eq 0x4B) { $blob[2] = 0x53 }
        try {
            [PrismSignKeys]::Import($name, $algorithm, $spec, $blobType, $blob)
            $provider = [PrismSignKeys]::Ksp
        } catch {
            $why = $_.Exception.InnerException.Message
        } finally {
            [Array]::Clear($blob, 0, $blob.Length)
        }
    }
    if (-not $provider -and $rsa) {
        # signtool signs with a CryptoAPI container's AT_KEYEXCHANGE key.
        $p = New-Object Security.Cryptography.CspParameters 24, ([PrismSignKeys]::Csp), $name
        $p.KeyNumber = 1
        $p.Flags = 'UseNonExportableKey, NoPrompt'
        $r = New-Object Security.Cryptography.RSACryptoServiceProvider $p
        $r.PersistKeyInCsp = $true
        $r.ImportParameters($rsa.ExportParameters($true))
        $r.Dispose()
        $provider = [PrismSignKeys]::Csp
    }
    if (-not $provider) { throw "importing the key into the $([PrismSignKeys]::Ksp) failed: $why" }
    [Console]::Out.WriteLine($provider)
} catch {
    $e = $_.Exception
    while ($e.InnerException) { $e = $e.InnerException }
    [Console]::Error.WriteLine($e.Message.Trim())
    exit 1
}
