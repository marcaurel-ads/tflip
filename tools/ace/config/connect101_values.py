#!/usr/bin/env python3
"""Writes connect101_values.txt: the connect string options (`key=value`, one
set per line) for run_connect101.sh to try, to see which values each option
of AdsConnect101 accepts.

    tools/ace/config/connect101_values.py > tools/ace/config/connect101_values.txt
"""
BOOLEANS = ["ReadOnly", "Shared", "ShowDeleted", "TrimTrailingSpaces", "Exact",
            "CheckFreeTableAccess", "IncrementUserCount", "Pooling", "FIPS"]
BOOLEAN_VALUES = ["true", "True", "TRUE", "false", "FALSE", "1", "0", "yes", "no", "t", "y",
                  "on", "off", " true "]
VALUES = {
    "TableType": ["adt", "Adt", "ADS_ADT", "ADS_CDX", "1", "2", "3", "4", "5", "VFP", "FOXPRO",
                  "CLIPPER", "DBF", "CDX ", " ntx"],
    "CharType": ["ansi", "Oem", "ADS_ANSI", "1", "2", "0"],
    "LockMode": ["compatible", "Proprietary", "ADS_COMPATIBLE_LOCKING",
                 "ADS_PROPRIETARY_LOCKING", "0", "1", "Advantage", "2"],
    "SecurityMode": ["checkrights", "Ignorerights", "ADS_CHECKRIGHTS", "ADS_IGNORERIGHTS",
                     "1", "2", "0"],
    "CommType": ["TCP_IP", "UDP_IP", "IPX", "TLS", "tcp_ip", "tls", "TCP", "SPX", "NETBIOS",
                 "INTERNET", "TCP_IP|UDP_IP", ""],
    "EncryptionType": ["RC4", "AES128", "AES256", "NONE", "rc4", "aes128", "ADS_ENCRYPT_RC4",
                       "1", "0", ""],
    "Compression": ["internet", "Always", "NEVER", "never", "1", ""],
    "ServerType": ["local", "REMOTE", "ais", "1", "2", "4", "7", "bogus", ""],
    "Epoch": ["0", "1900", "1950", "2000", "2100", "99999", "-1", "abc", "1950x", ""],
    "Decimals": ["0", "2", "5", "255", "256", "-1", "abc", ""],
    "SQLTimeout": ["0", "30", "-1", "99999999999", "abc", ""],
    "DateFormat": ["MM/DD/YYYY", "DD.MM.YYYY", "mm/dd/yyyy", "YYYY/MM/DD", "M/D/Y", "x",
                   "DD-MON-YYYY", "MM/DD/CCYY", ""],
    "Max Pool Size": ["0", "5", "-1", "abc", ""],
    "Connection Lifetime": ["0", "5", "-1", "abc", ""],
    "Connect Timeout": ["0", "5", "-1", "abc", ""],
    "Collation": ["", "ansi", "ASCII", "GERMAN_VFP_CI_AS_1252"],
}
OTHERS = ["TableType", "ReadOnly=", "=TRUE", "TableType=CDX;TableType=NTX",
          "TableType=NTX;TableType=CDX", "ReadOnly=TRUE;ReadOnly=FALSE", ";;TableType=CDX;;",
          "Bogus", "TableType=CDX;Bogus=1", "Bogus=1;TableType=CDX", "ReadOnly=zzz;Bogus=1",
          "Bogus=1;ReadOnly=zzz", "TableType=zzz;CharType=zzz", "CharType=zzz;TableType=zzz"]

for key in BOOLEANS:
    for value in BOOLEAN_VALUES:
        print(f"{key}={value}")
for key, values in VALUES.items():
    for value in values:
        print(f"{key}={value}")
for line in OTHERS:
    print(line)
