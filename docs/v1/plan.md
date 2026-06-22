> ⚠️ **Historical (v1) document.** This is the original v1 design sketch, kept
> for reference. It does **not** reflect the shipped implementation (e.g. it
> proposes Ed25519/Argon2/HASHSECRET and a `Sessions` table and `/pwd/list/valid`
> + `/pwd/list/expired` that were never built). For the current system see
> [`../API.md`](../API.md), [`../ARCHITECTURE.md`](../ARCHITECTURE.md), and
> [`../follow-4-test.md`](../follow-4-test.md).

# Password Manager 

## v1 Plan

This project is backend project for password managment on a server:

o Diesel
o diesel-cli
o SQLite
o tokio::fs
o Argon2
o AES-256-GCM
o Ed25519 signatures
o Axum

Server Side (Backend Rust)

.env
DATABASEENCRPYTSECRET   =  .... 256 bytes long
SOFTWARESECRET          =  .... 256 bytes long
HASHSECRET              =  .... 256 bytes long

Function

checkIpAndDeviceTokenMatchesWithSession(deviceToken: String, headerIp: String ) {
    identity = Identity.findUnique(token: deviceToken)
    match identity.ip === IP { OK() }
    Exception with STATUSCODE 401
}

createEncrpytPasswordWithPWDLogic(identityID: UUID, pwd: string, groupID: UUID | null) {
    let identity = Identity.findUnique(uuid: identityID)
    let private = identity.serverPrivateKey
    
    /// first we decrypt [pwd] with private key of server
    let encrpytPwd => encrpyt it with : DATABASEENCRPYTSECRET      
    let createdPwd = Passwords.create(pwd: encrpytPwd, groupID, identityID)
    let cryptEncrpytPwd = createdPwd -> we again encrpyt with identity.clientPublicKey -> encrpytPwd  
    
    return { pwd: cryptEncrpytPwd, createdAt: createdPwd.createdAt }
}

updateEncrpytPasswordWithPWDLogic(pwdID: UUID, pwd: string) {
    let pwdToUpdate = Passwords.findUnique(uuid: pwdID)
    let identity = Identity.findUnique(uuid: pwdToUpdate)
    let private = identity.serverPrivateKey

    /// first we decrypt [pwd] with private key of server
    let encrpytPwd => encrpyt it with : DATABASEENCRPYTSECRET      
    let createdPwd = Passwords.update(pwd: encrpytPwd)
    let cryptEncrpytPwd = createdPwd -> we again encrpyt with identity.clientPublicKey -> encrpytPwd  

    return { pwd: cryptEncrpytPwd, updatedAt: createdPwd.updatedAt }
}

o Endpoints

ALL ENDPOINTS REQUIRES IP

### Greet server for new clients
/greet
Header:
    ip: [IP] X-IP FORWARD
Payload:    
    pub: Stored public key from client

Get's IP that belongs to client and saves it to database encrpty which is provided at .env      
Create a private / public keys and saves for that [IP]
Leaves deviceToken null
But leaves isConfirmed field false for admin to approve ip's connect
Raises exception : 
if that ip is already in database: 412    
if pub key is already in database: 409
returns [PUBLICKEY], 200

### Register yourself, client sends out token & ehlo only accepted when no ip is register
/register
Header:
    ip:       [IP] X-IP FORWARD 

payload:
    token:    [DVC] device token which is encyprted with pub Key given
    ehlo:     [EHL] ehlo secret which is encyprted with pub Key given
    
    /// We check ip is in database
    let ipExists = Identity.findUnique(ipAddress:[ip])
    
    if ipExists -> statusCode === 403 
    
    return 200


### Used for syncing clients with dynamic IP, we register changed IP address by sending ehlo       
/re-sign
Header:
    ip:       [IP] X-IP FORWARD 

payload:
    token:    [DVC] device token which is encyprted with pri Key given
    ehlo:     [EHL] ehlo secret which is encyprted with pri Key given

Update my ip request:
Find Identity by device [token]
Approve matched decrypt [ehlo] with server private key
isConfirmed will be updated to false again waiting for admin approval 

### For refresh the device token of active client for sc issues
/refresh
Header:
    ip:       [IP] X-IP FORWARD 

payload:
    token:    [DVC] device token which is encyprted with pri Key given

    /// If token and ehlo is verified 
    Exception Status Code : 403

    let identity = Identity.findUnique(ipAddress: [IP])
    let deviceToken = identity.update(token)  /// We refresh deviceToken at database

return 200

### Verify that session is still valid for given device token and ip
/verify
Header:
    deviceToken: [Token] 256 bytes long from client's store
    ip: [IP]

Client checks with API that this ip and deviceToken is valid to connect
If not ask client to redirect to /re-sign

return 200

### List only passwords that are NOT expired
/pwd/list/valid
Header:
    deviceToken: [Token] 256 bytes long from client's store
    ip: [IP]

payload:
    take: Number = 1
    size: Number = 10
    search?: String | null 
    group?: String  | null = "ALL"
    sort?: Vec<Vec<String, "asc" | "desc">> = "createdAt"

    let queryID => First we find groupID by querying Groups only if group in not null
    
    We check passwords that are not expired which means
    queryExpires = Days(now() - createdAt) <  validSinceDays   
    
    We write query then:
    For each we iterate and add a meta data 
    uuid:               Key of Row
    pwd:                Hashed Password(Crpyt with pub key of client)
    expires:            How many days left to expire
    createdAt:          created at of row
    validSinceDays:     valid since of row
    
    return result, 200

### List only passwords that are expired
/pwd/list/expired
Header:
    deviceToken: [Token] 256 bytes long from client's store
    ip: [IP]
Payload:
    same with => /list/valid
    
    same with /list/valid but only we query Opposite(Days(now() - createdAt) <  validSinceDays) 
    return data, 200

### Create Password
/pwd/create
Header:
    deviceToken: [Token] 256 bytes long from client's store
    ip: [IP]

Payload:
    pwd: String /// client encrpyts it with client's public key
    groupID: Groups.uuid   
    extra?: JSON

    Check if that group and    
    return data, 200

### Update Password
/pwd/update/[UUID]
Header:
    deviceToken: [Token] 256 bytes long from client's store
    ip: [IP]
    extra?: JSON

Payload:
    pwdID: UUID
    pwd: String /// client encrpyts it with client's public key
    groupID: Groups.uuid   
    
    
    return 200

### Get a password detail with UUID
/pwd/get/[UUID]
Header:
    deviceToken: [Token] 256 bytes long from client's store
    ip: [IP]

    => A detail fetch with uuid

    o Fetch Group details
    name, extra

    o PWD details
    all fields except   groupID 
    expires:            How many days left to expire
    createdAt:          created at of row
    validSinceDays:     valid since of row
 
    return data, 200

Database Schema

o Groups
    
    uuid                UUID                  # primary key
    identityID          FK
    name                String
    extra               JSON
    
    createdAt           TIMESTAMP
    updatedAt           TIMESTAMP   

o Sessions

    uuid                UUID                  # primary key
    identityID          FK
    
    createdAt           TIMESTAMP
    updatedAt           TIMESTAMP

o Identity
    
    uuid                UUID                  # primary key
    ipAddress:          String[15]            # ip address of client
    deviceToken:        Unique[String]        # device Token that is sent by client
    ehloSecret:         String                # ehlo a secret message that client sends for any key loss
    serverPrivateKey:   Bytes                 # server uses this key to decrpyt client payload/messages
    serverPublicKey:    Bytes                 # client uses this key to encrpyt messages and send to server
    clientPublicKey:    Bytes                 # server uses this key to send encrypt messages to client
    extra:              JSON                  # company or organization like info as json    
    
    isConfirmed         Bool                  # confirm block that admin approves that this client can store
                                              # password to this server
    createdAt           TIMESTAMP
    updatedAt           TIMESTAMP

o Passwords

    uuid                UUID                  # primary key
    
    groupID             FK
    pwd                 String                # Encrypted with encrpytion logic libs above written as  
    
    name                String                # Human understandable name of  
    extra               JSON                  # Some information of where this password belongs

    createdAt           TIMESTAMP
    updatedAt           TIMESTAMP
    validSinceDays      Number

Gets Database

Gets .env KEYS
DATABASEENCRPYTSECRET   =  .... 256 bytes long
SOFTWARESECRET          =  .... 256 bytes long
HASHSECRET              =  .... 256 bytes long

Export  rsync -> send to a local machine
Import  rsync -> download to server machine



