// Сгенерировано crates/mrp/tools/schema.py из proto/*.proto: не править руками.
#![allow(non_upper_case_globals, non_snake_case)]

/// Номера полей: `field::DeviceInfoMessage::deviceUID`.
pub mod field {
    pub mod AVEndpointDescriptor {
        pub const name: u32 = 1;
        pub const uniqueIdentifier: u32 = 2;
        pub const isLocalEndpoint: u32 = 5;
        pub const instanceIdentifier: u32 = 6;
        pub const isProxyGroupPlayer: u32 = 7;
        pub const connectionType: u32 = 8;
        pub const canModifyGroupMembership: u32 = 9;
    }
    pub mod AVOutputDeviceDescriptor {
        pub const name: u32 = 1;
        pub const uniqueIdentifier: u32 = 2;
        pub const groupID: u32 = 3;
        pub const modelID: u32 = 4;
        pub const macAddress: u32 = 5;
        pub const canAccessRemoteAssets: u32 = 6;
        pub const isRemoteControllable: u32 = 7;
        pub const isGroupLeader: u32 = 8;
        pub const isGroupable: u32 = 9;
        pub const deviceType: u32 = 10;
        pub const deviceSubType: u32 = 11;
        pub const modelSpecificInfoData: u32 = 12;
        pub const batteryLevel: u32 = 13;
        pub const isLocalDevice: u32 = 14;
        pub const supportsExternalScreen: u32 = 15;
        pub const requiresAuthorization: u32 = 16;
        pub const shouldForceRemoteControlabillity: u32 = 17;
        pub const sourceInfo: u32 = 18;
        pub const isDeviceGroupable: u32 = 19;
        pub const canRelayCommunicationChannel: u32 = 20;
        pub const logicalDeviceID: u32 = 21;
        pub const isProxyGroupPlayer: u32 = 22;
        pub const firmwareVersion: u32 = 23;
        pub const volume: u32 = 24;
        pub const isVolumeControlAvailable: u32 = 25;
        pub const canAccessAppleMusic: u32 = 26;
        pub const canAccessiCloudMusicLibrary: u32 = 27;
        pub const groupContainsGroupLeader: u32 = 28;
        pub const supportsBufferedAirPlay: u32 = 29;
        pub const canPlayEncryptedProgressiveDownloadAssets: u32 = 30;
        pub const canFetchMediaDataFromSender: u32 = 31;
        pub const resentsOptimizedUserInterfaceWhenPlayingFetchedAudioOnlyAssets: u32 = 32;
        pub const isAirPlayReceiverSessionActive: u32 = 33;
        pub const parentGroupIdentifier: u32 = 34;
        pub const parentGroupContainsDiscoverableLeader: u32 = 35;
        pub const isAddedToHomeKit: u32 = 36;
        pub const volumeCapabilities: u32 = 37;
        pub const bluetoothID: u32 = 38;
        pub const supportsHAP: u32 = 39;
        pub const usingJSONProtocol: u32 = 40;
        pub const clusterCompositions: u32 = 41;
        pub const clusterType: u32 = 42;
        pub const primaryUID: u32 = 43;
        pub const configuredClusterSize: u32 = 44;
        pub const supportsRapportRemoteControlTransport: u32 = 45;
        pub const currentBluetoothListeningMode: u32 = 46;
        pub const availableBluetoothListeningModes: u32 = 47;
        pub const supportsMultiplayer: u32 = 48;
        pub const producesLowFidelityAudio: u32 = 49;
        pub const hostDeviceClass: u32 = 50;
        pub const airPlayGroupID: u32 = 51;
        pub const supportsSharePlayHandoff: u32 = 52;
        pub const distance: u32 = 53;
        pub const discoveredOnSameInfra: u32 = 54;
        pub const activatedClusterMembers: u32 = 55;
        pub const pickable: u32 = 56;
        pub const transportType: u32 = 57;
        pub const clusterID: u32 = 58;
        pub const isClusterLeader: u32 = 59;
        pub const isAppleAccessory: u32 = 60;
        pub const parentUniqueIdentifier: u32 = 61;
        pub const roomID: u32 = 62;
        pub const roomName: u32 = 63;
        pub const allClusterMembers: u32 = 64;
        pub const engageOnClusterActivate: u32 = 67;
    }
    pub mod AVOutputDeviceSourceInfo {
        pub const routingContextUID: u32 = 1;
        pub const multipleBuiltInDevices: u32 = 2;
    }
    pub mod AudioBuffer {
        pub const formatSettings: u32 = 1;
        pub const packetCapacity: u32 = 2;
        pub const maximumPacketSize: u32 = 3;
        pub const packetCount: u32 = 4;
        pub const contents: u32 = 5;
        pub const packetDescriptions: u32 = 6;
    }
    pub mod AudioDataBlock {
        pub const buffer: u32 = 1;
        pub const time: u32 = 2;
        pub const gain: u32 = 3;
    }
    pub mod AudioFadeMessage {
        pub const playerPath: u32 = 1;
        pub const fadeType: u32 = 2;
    }
    pub mod AudioFadeResponseMessage {
        pub const fadeDuration: u32 = 1;
    }
    pub mod AudioFormat {
        pub const tier: u32 = 1;
        pub const bitrate: u32 = 2;
        pub const sampleRate: u32 = 3;
        pub const bitDepth: u32 = 4;
        pub const codec: u32 = 5;
        pub const spatialized: u32 = 6;
        pub const multiChannel: u32 = 7;
        pub const channelLayout: u32 = 8;
        pub const audioChannelLayoutDescription: u32 = 9;
        pub const groupID: u32 = 10;
        pub const stableVariantID: u32 = 11;
    }
    pub mod AudioFormatSettings {
        pub const formatSettingsPlistData: u32 = 1;
    }
    pub mod AudioRoute {
        pub const r#type: u32 = 1;
        pub const name: u32 = 2;
        pub const supportsSpatialization: u32 = 3;
    }
    pub mod AudioStreamPacketDescription {
        pub const startOffset: u32 = 1;
        pub const variableFramesInPacket: u32 = 2;
        pub const dataByteSize: u32 = 3;
    }
    pub mod AudioTime {
        pub const timestamp: u32 = 1;
        pub const sampleRate: u32 = 2;
    }
    pub mod ClientUpdatesConfigMessage {
        pub const artworkUpdates: u32 = 1;
        pub const nowPlayingUpdates: u32 = 2;
        pub const volumeUpdates: u32 = 3;
        pub const keyboardUpdates: u32 = 4;
        pub const outputDeviceUpdates: u32 = 5;
    }
    pub mod CommandInfo {
        pub const command: u32 = 1;
        pub const enabled: u32 = 2;
        pub const active: u32 = 3;
        pub const preferredIntervals: u32 = 4;
        pub const localizedTitle: u32 = 5;
        pub const minimumRating: u32 = 6;
        pub const maximumRating: u32 = 7;
        pub const supportedRates: u32 = 8;
        pub const localizedShortTitle: u32 = 9;
        pub const repeatMode: u32 = 10;
        pub const shuffleMode: u32 = 11;
        pub const presentationStyle: u32 = 12;
        pub const skipInterval: u32 = 13;
        pub const numAvailableSkips: u32 = 14;
        pub const skipFrequency: u32 = 15;
        pub const canScrub: u32 = 16;
        pub const supportedPlaybackQueueTypes: u32 = 17;
        pub const supportedCustomQueueIdentifiers: u32 = 18;
        pub const supportedInsertionPositions: u32 = 19;
        pub const supportsSharedQueue: u32 = 20;
        pub const upNextItemCount: u32 = 21;
        pub const preferredPlaybackRate: u32 = 22;
        pub const supportedPlaybackSessionTypes: u32 = 23;
        pub const currentPlaybackSessionTypes: u32 = 24;
        pub const playbackSessionIdentifier: u32 = 25;
        pub const currentQueueEndAction: u32 = 26;
        pub const supportedEndQueueActions: u32 = 27;
        pub const disableReason: u32 = 28;
        pub const supportedPlaybackSessionIdentifiers: u32 = 29;
    }
    pub mod CommandOptions {
        pub const sourceId: u32 = 2;
        pub const mediaType: u32 = 3;
        pub const externalPlayerCommand: u32 = 4;
        pub const skipInterval: u32 = 5;
        pub const playbackRate: u32 = 6;
        pub const rating: u32 = 7;
        pub const negative: u32 = 8;
        pub const playbackPosition: u32 = 9;
        pub const repeatMode: u32 = 10;
        pub const shuffleMode: u32 = 11;
        pub const trackID: u32 = 12;
        pub const radioStationID: u32 = 13;
        pub const radioStationHash: u32 = 14;
        pub const systemAppPlaybackQueueData: u32 = 15;
        pub const destinationAppDisplayID: u32 = 16;
        pub const sendOptions: u32 = 17;
        pub const requestDefermentToPlaybackQueuePosition: u32 = 18;
        pub const contextID: u32 = 19;
        pub const shouldOverrideManuallyCuratedQueue: u32 = 20;
        pub const stationURL: u32 = 21;
        pub const shouldBeginRadioPlayback: u32 = 22;
        pub const playbackQueueInsertionPosition: u32 = 23;
        pub const contentItemID: u32 = 24;
        pub const playbackQueueOffset: u32 = 25;
        pub const playbackQueueDestinationOffset: u32 = 26;
        pub const languageOption: u32 = 27;
        pub const playbackQueueContext: u32 = 28;
        pub const insertAfterContentItemID: u32 = 29;
        pub const nowPlayingContentItemID: u32 = 30;
        pub const replaceIntent: u32 = 31;
    }
    pub mod ConfigureConnectionMessage {
        pub const groupID: u32 = 1;
    }
    pub mod ContentItem {
        pub const identifier: u32 = 1;
        pub const metadata: u32 = 2;
        pub const artworkData: u32 = 3;
        pub const info: u32 = 4;
        pub const availableLanguageOptions: u32 = 5;
        pub const currentLanguageOptions: u32 = 6;
        pub const parentIdentifier: u32 = 9;
        pub const ancestorIdentifier: u32 = 10;
        pub const queueIdentifier: u32 = 11;
        pub const requestIdentifier: u32 = 12;
        pub const artworkDataWidth: u32 = 13;
        pub const artworkDataHeight: u32 = 14;
    }
    pub mod ContentItemMetadata {
        pub const title: u32 = 1;
        pub const subtitle: u32 = 2;
        pub const isContainer: u32 = 3;
        pub const isPlayable: u32 = 4;
        pub const playbackProgress: u32 = 5;
        pub const albumName: u32 = 6;
        pub const trackArtistName: u32 = 7;
        pub const albumArtistName: u32 = 8;
        pub const directorName: u32 = 9;
        pub const seasonNumber: u32 = 10;
        pub const episodeNumber: u32 = 11;
        pub const releaseDate: u32 = 12;
        pub const playCount: u32 = 13;
        pub const duration: u32 = 14;
        pub const localizedContentRating: u32 = 15;
        pub const isExplicitItem: u32 = 16;
        pub const playlistType: u32 = 17;
        pub const radioStationType: u32 = 18;
        pub const artworkAvailable: u32 = 19;
        pub const infoAvailable: u32 = 21;
        pub const languageOptionsAvailable: u32 = 22;
        pub const numberOfSections: u32 = 23;
        pub const lyricsAvailable: u32 = 24;
        pub const editingStyleFlags: u32 = 25;
        pub const isStreamingContent: u32 = 26;
        pub const isCurrentlyPlaying: u32 = 27;
        pub const collectionIdentifier: u32 = 28;
        pub const profileIdentifier: u32 = 29;
        pub const startTime: u32 = 30;
        pub const artworkMIMEType: u32 = 31;
        pub const assetURLString: u32 = 32;
        pub const composer: u32 = 33;
        pub const discNumber: u32 = 34;
        pub const elapsedTime: u32 = 35;
        pub const genre: u32 = 36;
        pub const isAlwaysLive: u32 = 37;
        pub const playbackRate: u32 = 39;
        pub const chapterCount: u32 = 40;
        pub const totalDiscCount: u32 = 41;
        pub const totalTrackCount: u32 = 42;
        pub const trackNumber: u32 = 43;
        pub const contentIdentifier: u32 = 44;
        pub const isSharable: u32 = 46;
        pub const isLiked: u32 = 48;
        pub const isInWishList: u32 = 49;
        pub const radioStationIdentifier: u32 = 50;
        pub const radioStationName: u32 = 52;
        pub const radioStationString: u32 = 53;
        pub const iTunesStoreIdentifier: u32 = 54;
        pub const iTunesStoreSubscriptionIdentifier: u32 = 55;
        pub const iTunesStoreArtistIdentifier: u32 = 56;
        pub const iTunesStoreAlbumIdentifier: u32 = 57;
        pub const purchaseInfoData: u32 = 58;
        pub const defaultPlaybackRate: u32 = 59;
        pub const downloadState: u32 = 60;
        pub const downloadProgress: u32 = 61;
        pub const appMetricsData: u32 = 62;
        pub const seriesName: u32 = 63;
        pub const mediaType: u32 = 64;
        pub const mediaSubType: u32 = 65;
        pub const nowPlayingInfoData: u32 = 67;
        pub const userInfoData: u32 = 68;
        pub const isSteerable: u32 = 69;
        pub const artworkURL: u32 = 70;
        pub const lyricsURL: u32 = 71;
        pub const deviceSpecificUserInfoData: u32 = 72;
        pub const collectionInfoData: u32 = 73;
        pub const elapsedTimeTimestamp: u32 = 74;
        pub const inferredTimestamp: u32 = 75;
        pub const serviceIdentifier: u32 = 76;
        pub const artworkDataWidth: u32 = 77;
        pub const artworkDataHeight: u32 = 78;
        pub const currentPlaybackDateData: u32 = 79;
        pub const artworkIdentifier: u32 = 80;
        pub const isLoading: u32 = 81;
        pub const artworkURLTemplatesData: u32 = 82;
        pub const legacyUniqueIdentifier: u32 = 83;
        pub const episodeType: u32 = 84;
        pub const artworkFileURL: u32 = 85;
        pub const brandIdentifier: u32 = 86;
        pub const localizedDurationString: u32 = 87;
        pub const albumYear: u32 = 88;
        pub const songTraits: u32 = 89;
        pub const albumTraits: u32 = 90;
        pub const playlistTraits: u32 = 91;
        pub const preferredFormat: u32 = 92;
        pub const activeFormat: u32 = 93;
        pub const activeFormatJustification: u32 = 94;
        pub const formatTierPreference: u32 = 95;
        pub const audioRoute: u32 = 96;
        pub const alternativeFormats: u32 = 97;
    }
    pub mod CryptoPairingMessage {
        pub const pairingData: u32 = 1;
        pub const status: u32 = 2;
        pub const isRetrying: u32 = 3;
        pub const isUsingSystemPairing: u32 = 4;
        pub const state: u32 = 5;
    }
    pub mod DeviceInfoMessage {
        pub const uniqueIdentifier: u32 = 1;
        pub const name: u32 = 2;
        pub const localizedModelName: u32 = 3;
        pub const systemBuildVersion: u32 = 4;
        pub const applicationBundleIdentifier: u32 = 5;
        pub const applicationBundleVersion: u32 = 6;
        pub const protocolVersion: u32 = 7;
        pub const lastSupportedMessageType: u32 = 8;
        pub const supportsSystemPairing: u32 = 9;
        pub const allowsPairing: u32 = 10;
        pub const connected: u32 = 11;
        pub const systemMediaApplication: u32 = 12;
        pub const supportsACL: u32 = 13;
        pub const supportsSharedQueue: u32 = 14;
        pub const supportsExtendedMotion: u32 = 15;
        pub const bluetoothAddress: u32 = 16;
        pub const sharedQueueVersion: u32 = 17;
        pub const deviceUID: u32 = 19;
        pub const managedConfigDeviceID: u32 = 20;
        pub const deviceClass: u32 = 21;
        pub const logicalDeviceCount: u32 = 22;
        pub const tightlySyncedGroup: u32 = 23;
        pub const isProxyGroupPlayer: u32 = 24;
        pub const tightSyncUID: u32 = 25;
        pub const groupUID: u32 = 26;
        pub const groupName: u32 = 27;
        pub const groupedDevices: u32 = 28;
        pub const isGroupLeader: u32 = 29;
        pub const isAirplayActive: u32 = 30;
        pub const systemPodcastApplication: u32 = 31;
        pub const senderDefaultGroupUID: u32 = 32;
        pub const airplayReceivers: u32 = 33;
        pub const linkAgent: u32 = 34;
        pub const clusterID: u32 = 35;
        pub const clusterLeaderID: u32 = 36;
        pub const clusterType: u32 = 37;
        pub const isClusterAware: u32 = 38;
        pub const modelID: u32 = 39;
        pub const supportsMultiplayer: u32 = 40;
        pub const routingContextID: u32 = 41;
        pub const airPlayGroupID: u32 = 42;
        pub const systemBooksApplication: u32 = 43;
        pub const clusteredDevices: u32 = 44;
        pub const parentGroupContainsDiscoverableGroupLeader: u32 = 45;
        pub const groupContainsDiscoverableGroupLeader: u32 = 46;
        pub const lastKnownClusterType: u32 = 47;
        pub const supportsOutputContextSync: u32 = 49;
        pub const computerName: u32 = 50;
        pub const configuredClusterSize: u32 = 51;
        pub const preferredEncoding: u32 = 52;
    }
    pub mod GenericMessage {
        pub const key: u32 = 1;
        pub const value: u32 = 2;
    }
    pub mod GetVolumeMessage {
        pub const outputDeviceUID: u32 = 1;
    }
    pub mod GetVolumeResultMessage {
        pub const volume: u32 = 1;
    }
    pub mod KeyboardMessage {
        pub const state: u32 = 1;
        pub const attributes: u32 = 3;
        pub const encryptedTextCyphertext: u32 = 4;
    }
    pub mod LanguageOption {
        pub const r#type: u32 = 1;
        pub const languageTag: u32 = 2;
        pub const characteristics: u32 = 3;
        pub const displayName: u32 = 4;
        pub const identifier: u32 = 5;
    }
    pub mod LanguageOptionGroup {
        pub const allowEmptySelection: u32 = 1;
        pub const defaultLanguageOption: u32 = 2;
        pub const languageOptions: u32 = 3;
    }
    pub mod ModifyOutputContextRequestMessage {
        pub const r#type: u32 = 1;
        pub const addingDevices: u32 = 2;
        pub const removingDevices: u32 = 3;
        pub const settingDevices: u32 = 4;
        pub const clusterAwareAddingDevices: u32 = 5;
        pub const clusterAwareRemovingDevices: u32 = 6;
        pub const clusterAwareSettingDevices: u32 = 7;
    }
    pub mod NotificationMessage {
        pub const notification: u32 = 1;
        pub const userInfo: u32 = 2;
    }
    pub mod NowPlayingClient {
        pub const processIdentifier: u32 = 1;
        pub const bundleIdentifier: u32 = 2;
        pub const parentApplicationBundleIdentifier: u32 = 3;
        pub const processUserIdentifier: u32 = 4;
        pub const nowPlayingVisibility: u32 = 5;
        pub const displayName: u32 = 7;
        pub const bundleIdentifierHierarchys: u32 = 8;
    }
    pub mod NowPlayingInfo {
        pub const album: u32 = 1;
        pub const artist: u32 = 2;
        pub const duration: u32 = 3;
        pub const elapsedTime: u32 = 4;
        pub const playbackRate: u32 = 5;
        pub const repeatMode: u32 = 6;
        pub const shuffleMode: u32 = 7;
        pub const timestamp: u32 = 8;
        pub const title: u32 = 9;
        pub const uniqueIdentifier: u32 = 10;
        pub const isExplicitTrack: u32 = 11;
        pub const isMusicApp: u32 = 12;
        pub const radioStationIdentifier: u32 = 13;
        pub const radioStationHash: u32 = 14;
        pub const radioStationName: u32 = 15;
        pub const artworkDataDigest: u32 = 16;
        pub const isAlwaysLive: u32 = 17;
        pub const isAdvertisement: u32 = 18;
    }
    pub mod NowPlayingPlayer {
        pub const identifier: u32 = 1;
        pub const displayName: u32 = 2;
        pub const isDefaultPlayer: u32 = 3;
        pub const audioSessionType: u32 = 4;
        pub const mxSessionIDs: u32 = 5;
        pub const audioSessionID: u32 = 6;
        pub const iconURL: u32 = 7;
    }
    pub mod Origin {
        pub const r#type: u32 = 1;
        pub const displayName: u32 = 2;
        pub const identifier: u32 = 3;
        pub const deviceInfo: u32 = 4;
        pub const isLocallyHosted: u32 = 5;
    }
    pub mod OriginClientPropertiesMessage {
        pub const lastPlayingTimestamp: u32 = 1;
    }
    pub mod PlaybackQueue {
        pub const location: u32 = 1;
        pub const contentItems: u32 = 2;
        pub const context: u32 = 3;
        pub const requestId: u32 = 4;
        pub const resolvedPlayerPath: u32 = 5;
        pub const sendingPlaybackQueueTransaction: u32 = 6;
        pub const queueIdentifier: u32 = 7;
    }
    pub mod PlaybackQueueCapabilities {
        pub const requestByRange: u32 = 1;
        pub const requestByIdentifiers: u32 = 2;
        pub const requestByRequest: u32 = 3;
    }
    pub mod PlaybackQueueContext {
        pub const revision: u32 = 1;
    }
    pub mod PlaybackQueueRequestMessage {
        pub const location: u32 = 1;
        pub const length: u32 = 2;
        pub const includeMetadata: u32 = 3;
        pub const artworkWidth: u32 = 4;
        pub const artworkHeight: u32 = 5;
        pub const includeLyrics: u32 = 6;
        pub const includeSections: u32 = 7;
        pub const includeInfo: u32 = 8;
        pub const includeLanguageOptions: u32 = 9;
        pub const context: u32 = 10;
        pub const requestID: u32 = 11;
        pub const contentItemIdentifiers: u32 = 12;
        pub const returnContentItemAssetsInUserCompletion: u32 = 13;
        pub const playerPath: u32 = 14;
        pub const cachingPolicy: u32 = 15;
        pub const label: u32 = 16;
        pub const isLegacyNowPlayingInfoRequest: u32 = 17;
    }
    pub mod PlayerClientPropertiesMessage {
        pub const playerPath: u32 = 1;
        pub const lastPlayingTimestamp: u32 = 2;
    }
    pub mod PlayerPath {
        pub const origin: u32 = 1;
        pub const client: u32 = 2;
        pub const player: u32 = 3;
    }
    pub mod PreloadedPlaybackSessionInfo {
        pub const playbackSessionIdentifier: u32 = 1;
        pub const playbackSessionRevision: u32 = 2;
        pub const playbackSessionPriority: u32 = 3;
    }
    pub mod ProtocolMessage {
        pub const audioFadeMessage: u32 = 88;
        pub const audioFadeResponseMessage: u32 = 89;
        pub const clientUpdatesConfigMessage: u32 = 21;
        pub const configureConnectionMessage: u32 = 94;
        pub const cryptoPairingMessage: u32 = 39;
        pub const deviceInfoMessage: u32 = 20;
        pub const genericMessage: u32 = 46;
        pub const getKeyboardSessionMessage: u32 = 29;
        pub const getRemoteTextInputSessionMessage: u32 = 72;
        pub const getVolumeMessage: u32 = 53;
        pub const getVolumeResultMessage: u32 = 54;
        pub const keyboardMessage: u32 = 28;
        pub const modifyOutputContextRequestMessage: u32 = 52;
        pub const notificationMessage: u32 = 16;
        pub const originClientPropertiesMessage: u32 = 87;
        pub const playbackQueueRequestMessage: u32 = 37;
        pub const playerClientPropertiesMessage: u32 = 86;
        pub const r#type: u32 = 1;
        pub const identifier: u32 = 2;
        pub const authenticationToken: u32 = 3;
        pub const errorCode: u32 = 4;
        pub const timestamp: u32 = 5;
        pub const errorDescription: u32 = 78;
        pub const uniqueIdentifier: u32 = 85;
        pub const registerForGameControllerEventsMessage: u32 = 27;
        pub const registerHIDDeviceMessage: u32 = 11;
        pub const registerHIDDeviceResultMessage: u32 = 12;
        pub const registerVoiceInputDeviceMessage: u32 = 33;
        pub const registerVoiceInputDeviceResponseMessage: u32 = 34;
        pub const remoteTextInputMessage: u32 = 71;
        pub const removeClientMessage: u32 = 57;
        pub const removeEndpointsMessage: u32 = 84;
        pub const removeOutputDevicesMessage: u32 = 70;
        pub const removePlayerMessage: u32 = 58;
        pub const sendButtonEventMessage: u32 = 43;
        pub const sendCommandMessage: u32 = 6;
        pub const sendCommandResultMessage: u32 = 7;
        pub const sendHIDEventMessage: u32 = 13;
        pub const sendPackedVirtualTouchEventMessage: u32 = 47;
        pub const sendVoiceInputMessage: u32 = 36;
        pub const setArtworkMessage: u32 = 10;
        pub const setConnectionStateMessage: u32 = 42;
        pub const setDefaultSupportedCommandsMessage: u32 = 75;
        pub const setDiscoveryModeMessage: u32 = 82;
        pub const setHiliteModeMessage: u32 = 44;
        pub const setNowPlayingClientMessage: u32 = 50;
        pub const setNowPlayingPlayerMessage: u32 = 51;
        pub const setRecordingStateMessage: u32 = 35;
        pub const setStateMessage: u32 = 9;
        pub const setVolumeMessage: u32 = 55;
        pub const textInputMessage: u32 = 30;
        pub const transactionMessage: u32 = 38;
        pub const updateClientMessage: u32 = 59;
        pub const updateContentItemArtworkMessage: u32 = 61;
        pub const updateContentItemMessage: u32 = 60;
        pub const updateEndPointsMessage: u32 = 83;
        pub const updateOutputDeviceMessage: u32 = 69;
        pub const updatePlayerMessage: u32 = 62;
        pub const volumeControlAvailabilityMessage: u32 = 22;
        pub const volumeControlCapabilitiesDidChangeMessage: u32 = 68;
        pub const volumeDidChangeMessage: u32 = 56;
        pub const wakeDeviceMessage: u32 = 45;
    }
    pub mod RegisterForGameControllerEventsMessage {
        pub const inputModeFlags: u32 = 1;
    }
    pub mod RegisterHIDDeviceMessage {
        pub const deviceDescriptor: u32 = 1;
    }
    pub mod RegisterHIDDeviceResultMessage {
        pub const errorCode: u32 = 1;
        pub const deviceIdentifier: u32 = 2;
    }
    pub mod RegisterVoiceInputDeviceMessage {
        pub const deviceDescriptor: u32 = 1;
    }
    pub mod RegisterVoiceInputDeviceResponseMessage {
        pub const deviceID: u32 = 1;
        pub const errorCode: u32 = 2;
    }
    pub mod RemoteTextInputMessage {
        pub const timestamp: u32 = 1;
        pub const version: u32 = 2;
        pub const data: u32 = 3;
    }
    pub mod RemoveClientMessage {
        pub const client: u32 = 1;
    }
    pub mod RemoveEndpointsMessage {
        pub const endpointUIDs: u32 = 1;
    }
    pub mod RemoveOutputDevicesMessage {
        pub const outputDeviceUIDs: u32 = 1;
        pub const endpointUID: u32 = 2;
    }
    pub mod RemovePlayerMessage {
        pub const playerPath: u32 = 1;
    }
    pub mod SendButtonEventMessage {
        pub const usagePage: u32 = 1;
        pub const usage: u32 = 2;
        pub const buttonDown: u32 = 3;
    }
    pub mod SendCommandMessage {
        pub const command: u32 = 1;
        pub const options: u32 = 2;
        pub const playerPath: u32 = 3;
    }
    pub mod SendCommandResult {
        pub const playerPath: u32 = 1;
        pub const sendError: u32 = 2;
        pub const statuses: u32 = 3;
    }
    pub mod SendCommandResultMessage {
        pub const sendError: u32 = 1;
        pub const handlerReturnStatus: u32 = 2;
        pub const handlerReturnStatusDatas: u32 = 3;
        pub const commandID: u32 = 4;
        pub const playerPath: u32 = 5;
        pub const commandResult: u32 = 6;
    }
    pub mod SendCommandResultStatus {
        pub const statusCode: u32 = 1;
        pub const r#type: u32 = 2;
        pub const customData: u32 = 5;
        pub const customDataType: u32 = 6;
    }
    pub mod SendHIDEventMessage {
        pub const hidEventData: u32 = 1;
    }
    pub mod SendPackedVirtualTouchEventMessage {
        pub const data: u32 = 1;
    }
    pub mod SendVoiceInputMessage {
        pub const dataBlock: u32 = 1;
    }
    pub mod SetArtworkMessage {
        pub const jpegData: u32 = 1;
    }
    pub mod SetConnectionStateMessage {
        pub const state: u32 = 1;
    }
    pub mod SetDefaultSupportedCommandsMessage {
        pub const nowPlayingInfo: u32 = 1;
        pub const supportedCommands: u32 = 2;
        pub const playbackQueue: u32 = 3;
        pub const displayID: u32 = 4;
        pub const displayName: u32 = 5;
        pub const playbackState: u32 = 6;
        pub const playbackQueueCapabilities: u32 = 8;
        pub const playerPath: u32 = 9;
        pub const request: u32 = 10;
        pub const playbackStateTimestamp: u32 = 11;
    }
    pub mod SetDiscoveryModeMessage {
        pub const mode: u32 = 1;
        pub const features: u32 = 2;
    }
    pub mod SetHiliteModeMessage {
        pub const hiliteMode: u32 = 1;
    }
    pub mod SetNowPlayingClientMessage {
        pub const client: u32 = 1;
    }
    pub mod SetNowPlayingPlayerMessage {
        pub const playerPath: u32 = 1;
    }
    pub mod SetRecordingStateMessage {
        pub const state: u32 = 1;
    }
    pub mod SetStateMessage {
        pub const nowPlayingInfo: u32 = 1;
        pub const supportedCommands: u32 = 2;
        pub const playbackQueue: u32 = 3;
        pub const displayID: u32 = 4;
        pub const displayName: u32 = 5;
        pub const playbackState: u32 = 6;
        pub const playbackQueueCapabilities: u32 = 8;
        pub const playerPath: u32 = 9;
        pub const request: u32 = 10;
        pub const playbackStateTimestamp: u32 = 11;
    }
    pub mod SetVolumeMessage {
        pub const volume: u32 = 1;
        pub const outputDeviceUID: u32 = 2;
    }
    pub mod SupportedCommands {
        pub const supportedCommands: u32 = 1;
    }
    pub mod TextEditingAttributes {
        pub const title: u32 = 1;
        pub const prompt: u32 = 2;
        pub const inputTraits: u32 = 3;
    }
    pub mod TextInputMessage {
        pub const timestamp: u32 = 1;
        pub const text: u32 = 2;
        pub const actionType: u32 = 3;
    }
    pub mod TextInputTraits {
        pub const autocapitalizationType: u32 = 1;
        pub const keyboardType: u32 = 2;
        pub const returnKeyType: u32 = 3;
        pub const autocorrection: u32 = 4;
        pub const spellchecking: u32 = 5;
        pub const enablesReturnKeyAutomatically: u32 = 6;
        pub const secureTextEntry: u32 = 7;
        pub const validTextRangeLocation: u32 = 8;
        pub const validTextRangeLength: u32 = 9;
        pub const pINEntrySeparatorIndexes: u32 = 10;
    }
    pub mod TransactionKey {
        pub const identifier: u32 = 1;
        pub const userData: u32 = 2;
    }
    pub mod TransactionMessage {
        pub const name: u32 = 1;
        pub const packets: u32 = 2;
        pub const playerPath: u32 = 3;
    }
    pub mod TransactionPacket {
        pub const key: u32 = 1;
        pub const packetData: u32 = 2;
        pub const identifier: u32 = 3;
        pub const totalLength: u32 = 4;
        pub const totalWritePosition: u32 = 5;
    }
    pub mod TransactionPackets {
        pub const packets: u32 = 1;
    }
    pub mod UpdateClientMessage {
        pub const client: u32 = 1;
    }
    pub mod UpdateContentItemArtworkMessage {
        pub const contentItems: u32 = 1;
        pub const playerPath: u32 = 2;
    }
    pub mod UpdateContentItemMessage {
        pub const contentItems: u32 = 1;
        pub const playerPath: u32 = 2;
    }
    pub mod UpdateEndPointsMessage {
        pub const endpoints: u32 = 1;
        pub const endpointFeatures: u32 = 2;
    }
    pub mod UpdateOutputDeviceMessage {
        pub const outputDevices: u32 = 1;
        pub const endpointUID: u32 = 2;
        pub const clusterAwareOutputDevices: u32 = 3;
    }
    pub mod UpdatePlayerMessage {
        pub const playerPath: u32 = 1;
    }
    pub mod VirtualTouchDeviceDescriptor {
        pub const absolute: u32 = 1;
        pub const integratedDisplay: u32 = 2;
        pub const screenSizeWidth: u32 = 3;
        pub const screenSizeHeight: u32 = 4;
    }
    pub mod VoiceInputDeviceDescriptor {
        pub const defaultFormat: u32 = 1;
        pub const supportedFormats: u32 = 2;
    }
    pub mod VolumeControlAvailabilityMessage {
        pub const volumeControlAvailable: u32 = 1;
        pub const volumeCapabilities: u32 = 2;
    }
    pub mod VolumeControlCapabilitiesDidChangeMessage {
        pub const capabilities: u32 = 1;
        pub const endpointUID: u32 = 3;
        pub const outputDeviceUID: u32 = 4;
    }
    pub mod VolumeDidChangeMessage {
        pub const volume: u32 = 1;
        pub const endpointUID: u32 = 2;
        pub const outputDeviceUID: u32 = 3;
    }
}

/// Типы `ProtocolMessage`.
pub mod kind {
    pub const UNKNOWN_MESSAGE: u32 = 0;
    pub const SEND_COMMAND_MESSAGE: u32 = 1;
    pub const SEND_COMMAND_RESULT_MESSAGE: u32 = 2;
    pub const GET_STATE_MESSAGE: u32 = 3;
    pub const SET_STATE_MESSAGE: u32 = 4;
    pub const SET_ARTWORK_MESSAGE: u32 = 5;
    pub const REGISTER_HID_DEVICE_MESSAGE: u32 = 6;
    pub const REGISTER_HID_DEVICE_RESULT_MESSAGE: u32 = 7;
    pub const SEND_HID_EVENT_MESSAGE: u32 = 8;
    pub const SEND_HID_REPORT_MESSAGE: u32 = 9;
    pub const SEND_VIRTUAL_TOUCH_EVENT_MESSAGE: u32 = 10;
    pub const NOTIFICATION_MESSAGE: u32 = 11;
    pub const CONTENT_ITEMS_CHANGED_NOTIFICATION_MESSAGE: u32 = 12;
    pub const DEVICE_INFO_MESSAGE: u32 = 15;
    pub const CLIENT_UPDATES_CONFIG_MESSAGE: u32 = 16;
    pub const VOLUME_CONTROL_AVAILABILITY_MESSAGE: u32 = 17;
    pub const GAME_CONTROLLER_MESSAGE: u32 = 18;
    pub const REGISTER_GAME_CONTROLLER_MESSAGE: u32 = 19;
    pub const REGISTER_GAME_CONTROLLER_RESPONSE_MESSAGE: u32 = 20;
    pub const UNREGISTER_GAME_CONTROLLER_MESSAGE: u32 = 21;
    pub const REGISTER_FOR_GAME_CONTROLLER_EVENTS_MESSAGE: u32 = 22;
    pub const KEYBOARD_MESSAGE: u32 = 23;
    pub const GET_KEYBOARD_SESSION_MESSAGE: u32 = 24;
    pub const TEXT_INPUT_MESSAGE: u32 = 25;
    pub const GET_VOICE_INPUT_DEVICES_MESSAGE: u32 = 26;
    pub const GET_VOICE_INPUT_DEVICES_RESPONSE_MESSAGE: u32 = 27;
    pub const REGISTER_VOICE_INPUT_DEVICE_MESSAGE: u32 = 28;
    pub const REGISTER_VOICE_INPUT_DEVICE_RESPONSE_MESSAGE: u32 = 29;
    pub const SET_RECORDING_STATE_MESSAGE: u32 = 30;
    pub const SEND_VOICE_INPUT_MESSAGE: u32 = 31;
    pub const PLAYBACK_QUEUE_REQUEST_MESSAGE: u32 = 32;
    pub const TRANSACTION_MESSAGE: u32 = 33;
    pub const CRYPTO_PAIRING_MESSAGE: u32 = 34;
    pub const GAME_CONTROLLER_PROPERTIES_MESSAGE: u32 = 35;
    pub const SET_READY_STATE_MESSAGE: u32 = 36;
    pub const DEVICE_INFO_UPDATE_MESSAGE: u32 = 37;
    pub const SET_CONNECTION_STATE_MESSAGE: u32 = 38;
    pub const SEND_BUTTON_EVENT_MESSAGE: u32 = 39;
    pub const SET_HILITE_MODE_MESSAGE: u32 = 40;
    pub const WAKE_DEVICE_MESSAGE: u32 = 41;
    pub const GENERIC_MESSAGE: u32 = 42;
    pub const SEND_PACKED_VIRTUAL_TOUCH_EVENT_MESSAGE: u32 = 43;
    pub const SEND_LYRICS_EVENT: u32 = 44;
    pub const SET_NOW_PLAYING_CLIENT_MESSAGE: u32 = 46;
    pub const SET_NOW_PLAYING_PLAYER_MESSAGE: u32 = 47;
    pub const MODIFY_OUTPUT_CONTEXT_REQUEST_MESSAGE: u32 = 48;
    pub const GET_VOLUME_MESSAGE: u32 = 49;
    pub const GET_VOLUME_RESULT_MESSAGE: u32 = 50;
    pub const SET_VOLUME_MESSAGE: u32 = 51;
    pub const VOLUME_DID_CHANGE_MESSAGE: u32 = 52;
    pub const REMOVE_CLIENT_MESSAGE: u32 = 53;
    pub const REMOVE_PLAYER_MESSAGE: u32 = 54;
    pub const UPDATE_CLIENT_MESSAGE: u32 = 55;
    pub const UPDATE_CONTENT_ITEM_MESSAGE: u32 = 56;
    pub const UPDATE_CONTENT_ITEM_ARTWORK_MESSAGE: u32 = 57;
    pub const UPDATE_PLAYER_MESSAGE: u32 = 58;
    pub const PROMPT_FOR_ROUTE_AUTHORIZATION_MESSAGE: u32 = 59;
    pub const PROMPT_FOR_ROUTE_AUTHORIZATION_RESPONSE_MESSAGE: u32 = 60;
    pub const PRESENT_ROUTE_AUTHORIZATION_STATUS_MESSAGE: u32 = 61;
    pub const GET_VOLUME_CONTROL_CAPABILITIES_MESSAGE: u32 = 62;
    pub const GET_VOLUME_CONTROL_CAPABILITIES_RESULT_MESSAGE: u32 = 63;
    pub const VOLUME_CONTROL_CAPABILITIES_DID_CHANGE_MESSAGE: u32 = 64;
    pub const UPDATE_OUTPUT_DEVICE_MESSAGE: u32 = 65;
    pub const REMOVE_OUTPUT_DEVICES_MESSAGE: u32 = 66;
    pub const REMOTE_TEXT_INPUT_MESSAGE: u32 = 67;
    pub const GET_REMOTE_TEXT_INPUT_SESSION_MESSAGE: u32 = 68;
    pub const REMOVE_OUTPUT_DEVICES_MESSAGE2: u32 = 69;
    pub const PLAYBACK_SESSION_REQUEST_MESSAGE: u32 = 70;
    pub const PLAYBACK_SESSION_RESPONSE_MESSAGE: u32 = 71;
    pub const SET_DEFAULT_SUPPORTED_COMMANDS_MESSAGE: u32 = 72;
    pub const PLAYBACK_SESSION_MIGRATE_REQUEST_MESSAGE: u32 = 73;
    pub const PLAYBACK_SESSION_MIGRATE_RESPONSE_MESSAGE: u32 = 74;
    pub const PLAYBACK_SESSION_MIGRATE_BEGIN_MESSAGE: u32 = 75;
    pub const PLAYBACK_SESSION_MIGRATE_END_MESSAGE: u32 = 76;
    pub const UPDATE_ACTIVE_SYSTEM_ENDPOINT_MESSAGE: u32 = 77;
    pub const SET_DISCOVERY_MODE_MESSAGE: u32 = 101;
    pub const UPDATE_END_POINTS_MESSAGE: u32 = 102;
    pub const REMOVE_ENDPOINTS_MESSAGE: u32 = 103;
    pub const PLAYER_CLIENT_PROPERTIES_MESSAGE: u32 = 104;
    pub const ORIGIN_CLIENT_PROPERTIES_MESSAGE: u32 = 105;
    pub const AUDIO_FADE_MESSAGE: u32 = 106;
    pub const AUDIO_FADE_RESPONSE_MESSAGE: u32 = 107;
    pub const CONFIGURE_CONNECTION_MESSAGE: u32 = 120;
}

/// Поле `ProtocolMessage`, в котором лежит тело сообщения этого типа.
pub fn extension(kind: u32) -> Option<u32> {
    match kind {
        1 => Some(6),
        2 => Some(7),
        4 => Some(9),
        5 => Some(10),
        6 => Some(11),
        7 => Some(12),
        8 => Some(13),
        11 => Some(16),
        15 => Some(20),
        16 => Some(21),
        17 => Some(22),
        22 => Some(27),
        23 => Some(28),
        24 => Some(29),
        25 => Some(30),
        28 => Some(33),
        29 => Some(34),
        30 => Some(35),
        31 => Some(36),
        32 => Some(37),
        33 => Some(38),
        34 => Some(39),
        37 => Some(20),
        38 => Some(42),
        39 => Some(43),
        40 => Some(44),
        41 => Some(45),
        42 => Some(46),
        43 => Some(47),
        46 => Some(50),
        47 => Some(51),
        48 => Some(52),
        49 => Some(53),
        50 => Some(54),
        51 => Some(55),
        52 => Some(56),
        53 => Some(57),
        54 => Some(58),
        55 => Some(59),
        56 => Some(60),
        57 => Some(61),
        58 => Some(62),
        64 => Some(68),
        65 => Some(69),
        66 => Some(70),
        67 => Some(71),
        68 => Some(72),
        72 => Some(75),
        101 => Some(82),
        102 => Some(83),
        103 => Some(84),
        104 => Some(86),
        105 => Some(87),
        106 => Some(88),
        107 => Some(89),
        120 => Some(94),
        _ => None,
    }
}

/// Значения перечислений: `value::SendError::NotSupported`.
pub mod value {
    pub mod ActionType {
        pub const Unknown: u64 = 0;
        pub const Insert: u64 = 1;
        pub const Set: u64 = 2;
        pub const Delete: u64 = 3;
        pub const ClearAction: u64 = 4;
    }
    pub mod ActiveFormatJustification {
        pub const Unknown: u64 = 0;
        pub const Unavailable: u64 = 1;
        pub const UserPreference: u64 = 100;
        pub const UserDownload: u64 = 101;
        pub const RouteIncompatible: u64 = 500;
        pub const RouteUnknownCompatibility: u64 = 501;
        pub const BandwidthInsufficient: u64 = 1000;
    }
    pub mod AlbumTraits {
        pub const None: u64 = 0;
        pub const AppleDigitialMaster: u64 = 1;
        pub const Lossless: u64 = 2;
        pub const HighResolutionLossless: u64 = 4;
        pub const Spatial: u64 = 8;
        pub const Atmos: u64 = 16;
        pub const Surround: u64 = 32;
    }
    pub mod AudioRouteType {
        pub const Unknown: u64 = 0;
        pub const DevicesSpeaker: u64 = 1;
        pub const LineOut: u64 = 2;
        pub const Headphones: u64 = 3;
        pub const BluetoothHeadphones: u64 = 4;
        pub const BluetoothSpeaker: u64 = 5;
        pub const USBAudio: u64 = 6;
        pub const CarAudio: u64 = 7;
        pub const HDMI: u64 = 8;
        pub const AirPlay: u64 = 9;
    }
    pub mod AudioTier {
        pub const LowBandwidthStereo: u64 = 1;
        pub const HighQualityStereo: u64 = 2;
        pub const Lossless: u64 = 3;
        pub const HighResolutionLossless: u64 = 4;
        pub const Spatial: u64 = 5;
    }
    pub mod AutocapitalizationType {
        pub const None: u64 = 0;
        pub const Words: u64 = 1;
        pub const Sentences: u64 = 2;
        pub const AllCharacters: u64 = 3;
    }
    pub mod Command {
        pub const Unknown: u64 = 0;
        pub const Play: u64 = 1;
        pub const Pause: u64 = 2;
        pub const TogglePlayPause: u64 = 3;
        pub const Stop: u64 = 4;
        pub const NextTrack: u64 = 5;
        pub const PreviousTrack: u64 = 6;
        pub const AdvanceShuffleMode: u64 = 7;
        pub const AdvanceRepeatMode: u64 = 8;
        pub const BeginFastForward: u64 = 9;
        pub const EndFastForward: u64 = 10;
        pub const BeginRewind: u64 = 11;
        pub const EndRewind: u64 = 12;
        pub const Rewind15Seconds: u64 = 13;
        pub const FastForward15Seconds: u64 = 14;
        pub const Rewind30Seconds: u64 = 15;
        pub const FastForward30Seconds: u64 = 16;
        pub const SkipForward: u64 = 18;
        pub const SkipBackward: u64 = 19;
        pub const ChangePlaybackRate: u64 = 20;
        pub const RateTrack: u64 = 21;
        pub const LikeTrack: u64 = 22;
        pub const DislikeTrack: u64 = 23;
        pub const BookmarkTrack: u64 = 24;
        pub const SeekToPlaybackPosition: u64 = 45;
        pub const ChangeRepeatMode: u64 = 46;
        pub const ChangeShuffleMode: u64 = 47;
        pub const EnableLanguageOption: u64 = 53;
        pub const DisableLanguageOption: u64 = 54;
        pub const NextChapter: u64 = 25;
        pub const PreviousChapter: u64 = 26;
        pub const NextAlbum: u64 = 27;
        pub const PreviousAlbum: u64 = 28;
        pub const NextPlaylist: u64 = 29;
        pub const PreviousPlaylist: u64 = 30;
        pub const BanTrack: u64 = 31;
        pub const AddTrackToWishList: u64 = 32;
        pub const RemoveTrackFromWishList: u64 = 33;
        pub const NextInContext: u64 = 34;
        pub const PreviousInContext: u64 = 35;
        pub const ResetPlaybackTimeout: u64 = 41;
        pub const SetPlaybackQueue: u64 = 48;
        pub const AddNowPlayingItemToLibrary: u64 = 49;
        pub const CreateRadioStation: u64 = 50;
        pub const AddItemToLibrary: u64 = 51;
        pub const InsertIntoPlaybackQueue: u64 = 52;
        pub const ReorderPlaybackQueue: u64 = 55;
        pub const RemoveFromPlaybackQueue: u64 = 56;
        pub const PlayItemInPlaybackQueue: u64 = 57;
        pub const PrepareForSetQueue: u64 = 58;
        pub const SetPlaybackSession: u64 = 59;
        pub const PreloadedPlaybackSession: u64 = 60;
        pub const SetPriorityForPlaybackSession: u64 = 61;
        pub const DiscardPlaybackSession: u64 = 62;
        pub const Reshuffle: u64 = 63;
        pub const ChangeQueueEndAction: u64 = 135;
    }
    pub mod ContentItemMetadata {
        pub const UnknownMediaType: u64 = 0;
        pub const Audio: u64 = 1;
        pub const Video: u64 = 2;
        pub const UnknownMediaSubType: u64 = 0;
        pub const Music: u64 = 1;
        pub const Podcast: u64 = 4;
        pub const AudioBook: u64 = 5;
        pub const ITunesU: u64 = 6;
    }
    pub mod DeviceClass {
        pub const Invalid: u64 = 0;
        pub const iPhone: u64 = 1;
        pub const iPod: u64 = 2;
        pub const iPad: u64 = 3;
        pub const AppleTV: u64 = 4;
        pub const iFPGA: u64 = 5;
        pub const Watch: u64 = 6;
        pub const Accessory: u64 = 7;
        pub const Bridge: u64 = 8;
        pub const Mac: u64 = 9;
    }
    pub mod DeviceSubType {
        pub const Default: u64 = 0;
        pub const Speaker: u64 = 1;
        pub const Headphones: u64 = 2;
        pub const Headset: u64 = 3;
        pub const Receiver: u64 = 4;
        pub const LineOut: u64 = 5;
        pub const USB: u64 = 6;
        pub const DisplayPort: u64 = 7;
        pub const HDMI: u64 = 8;
        pub const LowEnergy: u64 = 9;
        pub const SPDIF: u64 = 10;
        pub const TV: u64 = 11;
        pub const HomePod: u64 = 12;
        pub const AppleTV: u64 = 13;
        pub const Vehicle: u64 = 14;
        pub const Cluster: u64 = 15;
        pub const SetTopBox: u64 = 16;
        pub const TVStick: u64 = 17;
    }
    pub mod DeviceType {
        pub const Unknown: u64 = 0;
        pub const AirPlay: u64 = 1;
        pub const Bluetooth: u64 = 2;
        pub const CarPlay: u64 = 3;
        pub const BuiltIn: u64 = 4;
        pub const Wired: u64 = 5;
    }
    pub mod DisableReason {
        pub const Unknown: u64 = 0;
        pub const AdPlayback: u64 = 1;
        pub const SkipLimitReached: u64 = 2;
    }
    pub mod ErrorCode {
        pub const NoError: u64 = 0;
        pub const UnknownError: u64 = 1;
        pub const InvalidOperation: u64 = 2;
        pub const OperationNotPermitted: u64 = 3;
        pub const ClientDoesNotExist: u64 = 4;
        pub const OriginDoesNotExist: u64 = 5;
        pub const UnsupportedOperation: u64 = 6;
        pub const FailedToSetPickedRoute: u64 = 7;
        pub const FailedToRegisterCustomOrigin: u64 = 8;
        pub const FailedToRemoveCustomOrigin: u64 = 9;
        pub const TheApplicationActivityDoesNotExist: u64 = 10;
        pub const TheAppHasNotSetupABrowsableContentEndpoint: u64 = 11;
        pub const TheRequestedBrowsableContentApiIsNotSupportedByTheApplication: u64 = 12;
        pub const TheNotficationHasNotBeenWhitelistedByTheServer: u64 = 13;
        pub const OperationRequiresAClientCallbackToHaveBeenRegistered: u64 = 14;
        pub const OperationRequiresAClientDataSourceToHaveBeenRegistered: u64 = 15;
        pub const RequestedDataIsOutOfDateAndShouldBeRequestedAgain: u64 = 16;
        pub const TheDevicesEnforcedVolumeLimitHasBeenExceeded: u64 = 17;
        pub const VolumeValueIsOutOfRange: u64 = 18;
        pub const VolumeIsAlreadyAtTheMaximumValue: u64 = 19;
        pub const VolumeIsAlreadyMuted: u64 = 20;
        pub const VoiceInputEndpointDoesNotExist: u64 = 21;
        pub const TheVoiceInputDeviceIsNotRegisteredOrDoesNotExist: u64 = 22;
        pub const EncryptionFailure: u64 = 23;
        pub const EndpointDoesNotExist: u64 = 24;
        pub const TheClientsApplicationCancelledTheOperation: u64 = 25;
        pub const TheOperationTimedOut: u64 = 26;
        pub const TheSpecifiedPlayerPathObjectWasInvalid: u64 = 27;
        pub const AddingOrRemovingDevicesFromTheAvOutputContextHasFailed: u64 = 28;
        pub const CouldNotFindTheSpecifiedNowPlayingPlayer: u64 = 29;
        pub const TheSpecifiedContentItemDoesNotExist: u64 = 30;
        pub const TheSpecifiedOffsetIsInvalid: u64 = 31;
        pub const TheSpecifiedOutputContextIsInvalid: u64 = 32;
        pub const OneOrMoreSpecifiedOutputDevicesAreNotGroupable: u64 = 33;
        pub const TheSpecifiedOutputContextDoesNotSupportAddingMoreThanOneOutputDevice: u64 = 34;
        pub const CouldNotFindTheSpecifiedNowPlayingClient: u64 = 35;
        pub const EndpointVolumeControlIsOnlyPossibleIfTheEndpointIsPickedOrRemoteControllable:
            u64 = 36;
        pub const OutputDeviceVolumeControlIsOnlyPossibleIfTheEndpointIsPickedOrRemoteControllable: u64 = 37;
        pub const CoderMustSupportKeyValueCoding: u64 = 38;
        pub const CouldNotFindTheGivenOutputdevice: u64 = 39;
        pub const FailedToConnectToRemoteDevice: u64 = 100;
        pub const AuthenticationTokenIsInvalid: u64 = 101;
        pub const RecordingSessionIsAlreadyInProgressOnThisDevice: u64 = 102;
        pub const TheDeviceIsNotCurrentlyRecording: u64 = 103;
        pub const TheClientHasDisconnected: u64 = 104;
        pub const TheServerHasDisconnected: u64 = 105;
        pub const TheConnectionHasBeenCancelledByTheClient: u64 = 106;
        pub const PairingFunctionalityIsLockedDueToSecurityReasons: u64 = 107;
        pub const TheClientsOperatingSystemVersionIsTooOld: u64 = 108;
        pub const TheClientsApplicationVersionIsTooOld: u64 = 109;
        pub const TheDeviceIsNotPaired: u64 = 110;
        pub const ThePinPairingDialogWasRemovedByTheUserBeforePairingOccoured: u64 = 111;
        pub const ThePinPairingDialogWasRemovedByATimeoutBeforePairingOccoured: u64 = 112;
        pub const TheConnectionTimedout: u64 = 113;
        pub const PairingWithThisDeviceIsBlocked: u64 = 114;
        pub const TheDeviceIsGoingToSleep: u64 = 115;
        pub const ConnectionBlockedByServer: u64 = 116;
        pub const MravendpointWasDeallocatedWhileWaitingForDeviceToConnect: u64 = 117;
        pub const OutputContextModificationCausedADeviceToNoLongerBeAProxyGroupPlayer: u64 = 200;
        pub const OutputContextModificationCausedADeviceToBecomeAProxyGroupPlayer: u64 = 201;
        pub const OutputContextModificationRequestedNoTopologyChange: u64 = 202;
        pub const OtherUnknownError: u64 = 299;
    }
    pub mod FormatTier {
        pub const LowBandwidthStereo: u64 = 1;
        pub const HighQualityStereo: u64 = 2;
        pub const Lossless: u64 = 4;
        pub const HighResolutionLossless: u64 = 8;
        pub const Spatial: u64 = 16;
    }
    pub mod HandlerReturnStatus {
        pub const Success: u64 = 0;
        pub const NoSuchContent: u64 = 1;
        pub const CommandFailed: u64 = 2;
        pub const NoActionableNowPlayingItem: u64 = 10;
        pub const DeviceNotFound: u64 = 20;
        pub const UIKitLegacy: u64 = 3;
        pub const SkipAdProhibited: u64 = 100;
        pub const QueueIsUserCurated: u64 = 101;
        pub const UserModifiedQueueDisabled: u64 = 102;
        pub const UserQueueModificationNotSupportedForCurrentItem: u64 = 103;
        pub const SubscriptionRequiredForSharedQueue: u64 = 104;
        pub const InsertionPositionNotSpecified: u64 = 105;
        pub const InvalidInsertionPosition: u64 = 106;
        pub const RequestParametersOutOfBounds: u64 = 107;
        pub const SkipLimitReached: u64 = 108;
        pub const AuthenticationFailure: u64 = 401;
        pub const MediaServiceUnavailable: u64 = 501;
    }
    pub mod KeyboardState {
        pub const Unknown: u64 = 0;
        pub const NotEditing: u64 = 1;
        pub const DidBeginEditing: u64 = 2;
        pub const Editing: u64 = 3;
        pub const TextDidChange: u64 = 4;
        pub const DidEndEditing: u64 = 5;
        pub const Response: u64 = 6;
    }
    pub mod KeyboardType {
        pub const Default: u64 = 0;
        pub const ASCII_Capable: u64 = 1;
        pub const NumbersAndPunctuation: u64 = 2;
        pub const URL: u64 = 3;
        pub const NumberPad: u64 = 4;
        pub const PhonePad: u64 = 5;
        pub const NamePhonePad: u64 = 6;
        pub const EmailAddress: u64 = 7;
        pub const DecimalPad: u64 = 8;
        pub const Twitter: u64 = 9;
        pub const WebSearch: u64 = 10;
        pub const Alphanet: u64 = 11;
        pub const PasscodePad: u64 = 12;
    }
    pub mod ModifyOutputContextRequestType {
        pub const SharedAudioPresentation: u64 = 1;
    }
    pub mod Origin {
        pub const Unknown: u64 = 0;
        pub const Local: u64 = 1;
        pub const Custom: u64 = 2;
    }
    pub mod PlaybackState {
        pub const Unknown: u64 = 0;
        pub const Playing: u64 = 1;
        pub const Paused: u64 = 2;
        pub const Stopped: u64 = 3;
        pub const Interrupted: u64 = 4;
        pub const Seeking: u64 = 5;
    }
    pub mod PlaylistTraits {
        pub const None: u64 = 0;
        pub const Spatial: u64 = 8;
        pub const Atmos: u64 = 16;
        pub const Surround: u64 = 32;
    }
    pub mod PreferredEncoding {
        pub const Default: u64 = 0;
        pub const JSON: u64 = 1;
    }
    pub mod QueueEndAction {
        pub const ClearAction: u64 = 0;
        pub const None: u64 = 1;
        pub const Reset: u64 = 2;
        pub const AutoPlay: u64 = 3;
    }
    pub mod RegisterForGameControllerEventsMessage {
        pub const None: u64 = 0;
        pub const Motion: u64 = 1;
        pub const Buttons: u64 = 2;
        pub const Digitizer: u64 = 3;
    }
    pub mod RepeatMode {
        pub const Unknown: u64 = 0;
        pub const Off: u64 = 1;
        pub const One: u64 = 2;
        pub const All: u64 = 3;
    }
    pub mod ReturnKeyType {
        pub const Default: u64 = 0;
        pub const Go: u64 = 1;
        pub const Google: u64 = 2;
        pub const Join: u64 = 3;
        pub const Next: u64 = 4;
        pub const Route: u64 = 5;
        pub const Search: u64 = 6;
        pub const Send: u64 = 7;
        pub const Yahoo: u64 = 8;
        pub const Done: u64 = 9;
        pub const EmergencyCall: u64 = 10;
        pub const Continue: u64 = 11;
    }
    pub mod SendCommandResultType {
        pub const Dialog: u64 = 1;
        pub const Error: u64 = 2;
        pub const Custom: u64 = 999;
    }
    pub mod SendCommandStatusCode {
        pub const Success: u64 = 0;
        pub const NoSuchContent: u64 = 1;
        pub const CommandFailed: u64 = 2;
        pub const NoActionableNowPlayingItem: u64 = 10;
        pub const DeviceNotFound: u64 = 20;
        pub const UIKitLegacy: u64 = 3;
        pub const SkipAdProhibited: u64 = 100;
        pub const QueueIsUserCurated: u64 = 101;
        pub const UserModifiedQueueDisabled: u64 = 102;
        pub const UserQueueModificationNotSupportedForCurrentItem: u64 = 103;
        pub const SubscriptionRequiredForSharedQueue: u64 = 104;
        pub const InsertionPositionNotSpecified: u64 = 105;
        pub const InvalidInsertionPosition: u64 = 106;
        pub const RequestParametersOutOfBounds: u64 = 107;
        pub const SkipLimitReached: u64 = 108;
        pub const AuthenticationFailure: u64 = 401;
        pub const MediaServiceUnavailable: u64 = 501;
    }
    pub mod SendError {
        pub const NoError: u64 = 0;
        pub const ApplicationNotFound: u64 = 1;
        pub const ConnectionFailed: u64 = 2;
        pub const Ignored: u64 = 3;
        pub const CouldNotLaunchApplication: u64 = 4;
        pub const TimedOut: u64 = 5;
        pub const OriginDoesNotExist: u64 = 6;
        pub const InvalidOptions: u64 = 7;
        pub const NoCommandHandlers: u64 = 8;
        pub const ApplicationNotInstalled: u64 = 9;
        pub const NotSupported: u64 = 10;
    }
    pub mod SendPackedVirtualTouchEventMessage {
        pub const Began: u64 = 1;
        pub const Moved: u64 = 2;
        pub const Stationary: u64 = 3;
        pub const Ended: u64 = 4;
        pub const Cancelled: u64 = 5;
    }
    pub mod SetConnectionStateMessage {
        pub const None: u64 = 0;
        pub const Connecting: u64 = 1;
        pub const Connected: u64 = 2;
        pub const Disconnected: u64 = 3;
    }
    pub mod SetRecordingStateMessage {
        pub const Unknown: u64 = 0;
        pub const Recording: u64 = 1;
        pub const NotRecording: u64 = 2;
    }
    pub mod ShuffleMode {
        pub const Unknown: u64 = 0;
        pub const Off: u64 = 1;
        pub const Albums: u64 = 2;
        pub const Songs: u64 = 3;
    }
    pub mod SongTraits {
        pub const None: u64 = 0;
        pub const AppleDigitialMaster: u64 = 1;
        pub const Lossless: u64 = 2;
        pub const HighResolutionLossless: u64 = 4;
        pub const Spatial: u64 = 8;
        pub const Atmos: u64 = 16;
        pub const Surround: u64 = 32;
    }
    pub mod VolumeCapabilities {
        pub const None: u64 = 0;
        pub const Relative: u64 = 1;
        pub const Absolute: u64 = 2;
        pub const Both: u64 = 3;
    }
}

/// Тип поля по проводу (0 varint, 1 fixed64, 2 длина, 5 fixed32) и
/// вложенное сообщение: для проверки того, что собрано.
pub fn wire_type(message: &str, field: u32) -> Option<(u8, &'static str)> {
    match (message, field) {
        ("AVEndpointDescriptor", 1) => Some((2, "")),
        ("AVEndpointDescriptor", 2) => Some((2, "")),
        ("AVEndpointDescriptor", 5) => Some((0, "")),
        ("AVEndpointDescriptor", 6) => Some((2, "")),
        ("AVEndpointDescriptor", 7) => Some((0, "")),
        ("AVEndpointDescriptor", 8) => Some((0, "")),
        ("AVEndpointDescriptor", 9) => Some((0, "")),
        ("AVOutputDeviceDescriptor", 1) => Some((2, "")),
        ("AVOutputDeviceDescriptor", 2) => Some((2, "")),
        ("AVOutputDeviceDescriptor", 3) => Some((2, "")),
        ("AVOutputDeviceDescriptor", 4) => Some((2, "")),
        ("AVOutputDeviceDescriptor", 5) => Some((2, "")),
        ("AVOutputDeviceDescriptor", 6) => Some((0, "")),
        ("AVOutputDeviceDescriptor", 7) => Some((0, "")),
        ("AVOutputDeviceDescriptor", 8) => Some((0, "")),
        ("AVOutputDeviceDescriptor", 9) => Some((0, "")),
        ("AVOutputDeviceDescriptor", 10) => Some((0, "")),
        ("AVOutputDeviceDescriptor", 11) => Some((0, "")),
        ("AVOutputDeviceDescriptor", 12) => Some((2, "")),
        ("AVOutputDeviceDescriptor", 13) => Some((5, "")),
        ("AVOutputDeviceDescriptor", 14) => Some((0, "")),
        ("AVOutputDeviceDescriptor", 15) => Some((0, "")),
        ("AVOutputDeviceDescriptor", 16) => Some((0, "")),
        ("AVOutputDeviceDescriptor", 17) => Some((0, "")),
        ("AVOutputDeviceDescriptor", 18) => Some((2, "AVOutputDeviceSourceInfo")),
        ("AVOutputDeviceDescriptor", 19) => Some((0, "")),
        ("AVOutputDeviceDescriptor", 20) => Some((0, "")),
        ("AVOutputDeviceDescriptor", 21) => Some((2, "")),
        ("AVOutputDeviceDescriptor", 22) => Some((0, "")),
        ("AVOutputDeviceDescriptor", 23) => Some((2, "")),
        ("AVOutputDeviceDescriptor", 24) => Some((5, "")),
        ("AVOutputDeviceDescriptor", 25) => Some((0, "")),
        ("AVOutputDeviceDescriptor", 26) => Some((0, "")),
        ("AVOutputDeviceDescriptor", 27) => Some((0, "")),
        ("AVOutputDeviceDescriptor", 28) => Some((0, "")),
        ("AVOutputDeviceDescriptor", 29) => Some((0, "")),
        ("AVOutputDeviceDescriptor", 30) => Some((0, "")),
        ("AVOutputDeviceDescriptor", 31) => Some((0, "")),
        ("AVOutputDeviceDescriptor", 32) => Some((0, "")),
        ("AVOutputDeviceDescriptor", 33) => Some((0, "")),
        ("AVOutputDeviceDescriptor", 34) => Some((2, "")),
        ("AVOutputDeviceDescriptor", 35) => Some((0, "")),
        ("AVOutputDeviceDescriptor", 36) => Some((0, "")),
        ("AVOutputDeviceDescriptor", 37) => Some((0, "")),
        ("AVOutputDeviceDescriptor", 38) => Some((2, "")),
        ("AVOutputDeviceDescriptor", 39) => Some((0, "")),
        ("AVOutputDeviceDescriptor", 40) => Some((0, "")),
        ("AVOutputDeviceDescriptor", 41) => Some((2, "AVOutputDeviceDescriptor")),
        ("AVOutputDeviceDescriptor", 42) => Some((0, "")),
        ("AVOutputDeviceDescriptor", 43) => Some((2, "")),
        ("AVOutputDeviceDescriptor", 44) => Some((0, "")),
        ("AVOutputDeviceDescriptor", 45) => Some((0, "")),
        ("AVOutputDeviceDescriptor", 46) => Some((2, "")),
        ("AVOutputDeviceDescriptor", 47) => Some((2, "")),
        ("AVOutputDeviceDescriptor", 48) => Some((0, "")),
        ("AVOutputDeviceDescriptor", 49) => Some((0, "")),
        ("AVOutputDeviceDescriptor", 50) => Some((0, "")),
        ("AVOutputDeviceDescriptor", 51) => Some((2, "")),
        ("AVOutputDeviceDescriptor", 52) => Some((0, "")),
        ("AVOutputDeviceDescriptor", 53) => Some((5, "")),
        ("AVOutputDeviceDescriptor", 54) => Some((0, "")),
        ("AVOutputDeviceDescriptor", 55) => Some((2, "AVOutputDeviceDescriptor")),
        ("AVOutputDeviceDescriptor", 56) => Some((0, "")),
        ("AVOutputDeviceDescriptor", 57) => Some((0, "")),
        ("AVOutputDeviceDescriptor", 58) => Some((2, "")),
        ("AVOutputDeviceDescriptor", 59) => Some((0, "")),
        ("AVOutputDeviceDescriptor", 60) => Some((0, "")),
        ("AVOutputDeviceDescriptor", 61) => Some((2, "")),
        ("AVOutputDeviceDescriptor", 62) => Some((2, "")),
        ("AVOutputDeviceDescriptor", 63) => Some((2, "")),
        ("AVOutputDeviceDescriptor", 64) => Some((2, "AVOutputDeviceDescriptor")),
        ("AVOutputDeviceDescriptor", 67) => Some((0, "")),
        ("AVOutputDeviceSourceInfo", 1) => Some((2, "")),
        ("AVOutputDeviceSourceInfo", 2) => Some((0, "")),
        ("AudioBuffer", 1) => Some((2, "AudioFormatSettings")),
        ("AudioBuffer", 2) => Some((0, "")),
        ("AudioBuffer", 3) => Some((0, "")),
        ("AudioBuffer", 4) => Some((0, "")),
        ("AudioBuffer", 5) => Some((2, "")),
        ("AudioBuffer", 6) => Some((2, "AudioStreamPacketDescription")),
        ("AudioDataBlock", 1) => Some((2, "AudioBuffer")),
        ("AudioDataBlock", 2) => Some((2, "AudioTime")),
        ("AudioDataBlock", 3) => Some((1, "")),
        ("AudioFadeMessage", 1) => Some((2, "PlayerPath")),
        ("AudioFadeMessage", 2) => Some((0, "")),
        ("AudioFadeResponseMessage", 1) => Some((0, "")),
        ("AudioFormat", 1) => Some((0, "")),
        ("AudioFormat", 2) => Some((0, "")),
        ("AudioFormat", 3) => Some((0, "")),
        ("AudioFormat", 4) => Some((0, "")),
        ("AudioFormat", 5) => Some((0, "")),
        ("AudioFormat", 6) => Some((0, "")),
        ("AudioFormat", 7) => Some((0, "")),
        ("AudioFormat", 8) => Some((0, "")),
        ("AudioFormat", 9) => Some((2, "")),
        ("AudioFormat", 10) => Some((2, "")),
        ("AudioFormat", 11) => Some((2, "")),
        ("AudioFormatSettings", 1) => Some((2, "")),
        ("AudioRoute", 1) => Some((2, "AudioRouteType")),
        ("AudioRoute", 2) => Some((2, "")),
        ("AudioRoute", 3) => Some((0, "")),
        ("AudioStreamPacketDescription", 1) => Some((0, "")),
        ("AudioStreamPacketDescription", 2) => Some((0, "")),
        ("AudioStreamPacketDescription", 3) => Some((0, "")),
        ("AudioTime", 1) => Some((1, "")),
        ("AudioTime", 2) => Some((1, "")),
        ("ClientUpdatesConfigMessage", 1) => Some((0, "")),
        ("ClientUpdatesConfigMessage", 2) => Some((0, "")),
        ("ClientUpdatesConfigMessage", 3) => Some((0, "")),
        ("ClientUpdatesConfigMessage", 4) => Some((0, "")),
        ("ClientUpdatesConfigMessage", 5) => Some((0, "")),
        ("CommandInfo", 1) => Some((0, "")),
        ("CommandInfo", 2) => Some((0, "")),
        ("CommandInfo", 3) => Some((0, "")),
        ("CommandInfo", 4) => Some((1, "")),
        ("CommandInfo", 5) => Some((2, "")),
        ("CommandInfo", 6) => Some((5, "")),
        ("CommandInfo", 7) => Some((5, "")),
        ("CommandInfo", 8) => Some((5, "")),
        ("CommandInfo", 9) => Some((2, "")),
        ("CommandInfo", 10) => Some((0, "")),
        ("CommandInfo", 11) => Some((0, "")),
        ("CommandInfo", 12) => Some((0, "")),
        ("CommandInfo", 13) => Some((0, "")),
        ("CommandInfo", 14) => Some((0, "")),
        ("CommandInfo", 15) => Some((0, "")),
        ("CommandInfo", 16) => Some((0, "")),
        ("CommandInfo", 17) => Some((0, "")),
        ("CommandInfo", 18) => Some((2, "")),
        ("CommandInfo", 19) => Some((0, "")),
        ("CommandInfo", 20) => Some((0, "")),
        ("CommandInfo", 21) => Some((0, "")),
        ("CommandInfo", 22) => Some((5, "")),
        ("CommandInfo", 23) => Some((2, "")),
        ("CommandInfo", 24) => Some((2, "")),
        ("CommandInfo", 25) => Some((2, "")),
        ("CommandInfo", 26) => Some((0, "")),
        ("CommandInfo", 27) => Some((0, "")),
        ("CommandInfo", 28) => Some((0, "")),
        ("CommandInfo", 29) => Some((2, "PreloadedPlaybackSessionInfo")),
        ("CommandOptions", 2) => Some((2, "")),
        ("CommandOptions", 3) => Some((2, "")),
        ("CommandOptions", 4) => Some((0, "")),
        ("CommandOptions", 5) => Some((5, "")),
        ("CommandOptions", 6) => Some((5, "")),
        ("CommandOptions", 7) => Some((5, "")),
        ("CommandOptions", 8) => Some((0, "")),
        ("CommandOptions", 9) => Some((1, "")),
        ("CommandOptions", 10) => Some((0, "")),
        ("CommandOptions", 11) => Some((0, "")),
        ("CommandOptions", 12) => Some((0, "")),
        ("CommandOptions", 13) => Some((0, "")),
        ("CommandOptions", 14) => Some((2, "")),
        ("CommandOptions", 15) => Some((2, "")),
        ("CommandOptions", 16) => Some((2, "")),
        ("CommandOptions", 17) => Some((0, "")),
        ("CommandOptions", 18) => Some((0, "")),
        ("CommandOptions", 19) => Some((2, "")),
        ("CommandOptions", 20) => Some((0, "")),
        ("CommandOptions", 21) => Some((2, "")),
        ("CommandOptions", 22) => Some((0, "")),
        ("CommandOptions", 23) => Some((0, "")),
        ("CommandOptions", 24) => Some((2, "")),
        ("CommandOptions", 25) => Some((0, "")),
        ("CommandOptions", 26) => Some((0, "")),
        ("CommandOptions", 27) => Some((2, "")),
        ("CommandOptions", 28) => Some((2, "")),
        ("CommandOptions", 29) => Some((2, "")),
        ("CommandOptions", 30) => Some((2, "")),
        ("CommandOptions", 31) => Some((0, "")),
        ("ConfigureConnectionMessage", 1) => Some((2, "")),
        ("ContentItem", 1) => Some((2, "")),
        ("ContentItem", 2) => Some((2, "ContentItemMetadata")),
        ("ContentItem", 3) => Some((2, "")),
        ("ContentItem", 4) => Some((2, "")),
        ("ContentItem", 5) => Some((2, "LanguageOptionGroup")),
        ("ContentItem", 6) => Some((2, "LanguageOption")),
        ("ContentItem", 9) => Some((2, "")),
        ("ContentItem", 10) => Some((2, "")),
        ("ContentItem", 11) => Some((2, "")),
        ("ContentItem", 12) => Some((2, "")),
        ("ContentItem", 13) => Some((0, "")),
        ("ContentItem", 14) => Some((0, "")),
        ("ContentItemMetadata", 1) => Some((2, "")),
        ("ContentItemMetadata", 2) => Some((2, "")),
        ("ContentItemMetadata", 3) => Some((0, "")),
        ("ContentItemMetadata", 4) => Some((0, "")),
        ("ContentItemMetadata", 5) => Some((5, "")),
        ("ContentItemMetadata", 6) => Some((2, "")),
        ("ContentItemMetadata", 7) => Some((2, "")),
        ("ContentItemMetadata", 8) => Some((2, "")),
        ("ContentItemMetadata", 9) => Some((2, "")),
        ("ContentItemMetadata", 10) => Some((0, "")),
        ("ContentItemMetadata", 11) => Some((0, "")),
        ("ContentItemMetadata", 12) => Some((1, "")),
        ("ContentItemMetadata", 13) => Some((0, "")),
        ("ContentItemMetadata", 14) => Some((1, "")),
        ("ContentItemMetadata", 15) => Some((2, "")),
        ("ContentItemMetadata", 16) => Some((0, "")),
        ("ContentItemMetadata", 17) => Some((0, "")),
        ("ContentItemMetadata", 18) => Some((0, "")),
        ("ContentItemMetadata", 19) => Some((0, "")),
        ("ContentItemMetadata", 21) => Some((0, "")),
        ("ContentItemMetadata", 22) => Some((0, "")),
        ("ContentItemMetadata", 23) => Some((0, "")),
        ("ContentItemMetadata", 24) => Some((0, "")),
        ("ContentItemMetadata", 25) => Some((0, "")),
        ("ContentItemMetadata", 26) => Some((0, "")),
        ("ContentItemMetadata", 27) => Some((0, "")),
        ("ContentItemMetadata", 28) => Some((2, "")),
        ("ContentItemMetadata", 29) => Some((2, "")),
        ("ContentItemMetadata", 30) => Some((1, "")),
        ("ContentItemMetadata", 31) => Some((2, "")),
        ("ContentItemMetadata", 32) => Some((2, "")),
        ("ContentItemMetadata", 33) => Some((2, "")),
        ("ContentItemMetadata", 34) => Some((0, "")),
        ("ContentItemMetadata", 35) => Some((1, "")),
        ("ContentItemMetadata", 36) => Some((2, "")),
        ("ContentItemMetadata", 37) => Some((0, "")),
        ("ContentItemMetadata", 39) => Some((5, "")),
        ("ContentItemMetadata", 40) => Some((0, "")),
        ("ContentItemMetadata", 41) => Some((0, "")),
        ("ContentItemMetadata", 42) => Some((0, "")),
        ("ContentItemMetadata", 43) => Some((0, "")),
        ("ContentItemMetadata", 44) => Some((2, "")),
        ("ContentItemMetadata", 46) => Some((0, "")),
        ("ContentItemMetadata", 48) => Some((0, "")),
        ("ContentItemMetadata", 49) => Some((0, "")),
        ("ContentItemMetadata", 50) => Some((0, "")),
        ("ContentItemMetadata", 52) => Some((2, "")),
        ("ContentItemMetadata", 53) => Some((2, "")),
        ("ContentItemMetadata", 54) => Some((0, "")),
        ("ContentItemMetadata", 55) => Some((0, "")),
        ("ContentItemMetadata", 56) => Some((0, "")),
        ("ContentItemMetadata", 57) => Some((0, "")),
        ("ContentItemMetadata", 58) => Some((2, "")),
        ("ContentItemMetadata", 59) => Some((5, "")),
        ("ContentItemMetadata", 60) => Some((0, "")),
        ("ContentItemMetadata", 61) => Some((5, "")),
        ("ContentItemMetadata", 62) => Some((2, "")),
        ("ContentItemMetadata", 63) => Some((2, "")),
        ("ContentItemMetadata", 64) => Some((0, "")),
        ("ContentItemMetadata", 65) => Some((0, "")),
        ("ContentItemMetadata", 67) => Some((2, "")),
        ("ContentItemMetadata", 68) => Some((2, "")),
        ("ContentItemMetadata", 69) => Some((0, "")),
        ("ContentItemMetadata", 70) => Some((2, "")),
        ("ContentItemMetadata", 71) => Some((2, "")),
        ("ContentItemMetadata", 72) => Some((2, "")),
        ("ContentItemMetadata", 73) => Some((2, "")),
        ("ContentItemMetadata", 74) => Some((1, "")),
        ("ContentItemMetadata", 75) => Some((1, "")),
        ("ContentItemMetadata", 76) => Some((2, "")),
        ("ContentItemMetadata", 77) => Some((0, "")),
        ("ContentItemMetadata", 78) => Some((0, "")),
        ("ContentItemMetadata", 79) => Some((2, "")),
        ("ContentItemMetadata", 80) => Some((2, "")),
        ("ContentItemMetadata", 81) => Some((0, "")),
        ("ContentItemMetadata", 82) => Some((2, "")),
        ("ContentItemMetadata", 83) => Some((0, "")),
        ("ContentItemMetadata", 84) => Some((0, "")),
        ("ContentItemMetadata", 85) => Some((2, "")),
        ("ContentItemMetadata", 86) => Some((2, "")),
        ("ContentItemMetadata", 87) => Some((2, "")),
        ("ContentItemMetadata", 88) => Some((2, "")),
        ("ContentItemMetadata", 89) => Some((0, "")),
        ("ContentItemMetadata", 90) => Some((0, "")),
        ("ContentItemMetadata", 91) => Some((0, "")),
        ("ContentItemMetadata", 92) => Some((2, "AudioFormat")),
        ("ContentItemMetadata", 93) => Some((2, "AudioFormat")),
        ("ContentItemMetadata", 94) => Some((0, "")),
        ("ContentItemMetadata", 95) => Some((0, "")),
        ("ContentItemMetadata", 96) => Some((2, "AudioRoute")),
        ("ContentItemMetadata", 97) => Some((2, "AudioFormat")),
        ("CryptoPairingMessage", 1) => Some((2, "")),
        ("CryptoPairingMessage", 2) => Some((0, "")),
        ("CryptoPairingMessage", 3) => Some((0, "")),
        ("CryptoPairingMessage", 4) => Some((0, "")),
        ("CryptoPairingMessage", 5) => Some((0, "")),
        ("DeviceInfoMessage", 1) => Some((2, "")),
        ("DeviceInfoMessage", 2) => Some((2, "")),
        ("DeviceInfoMessage", 3) => Some((2, "")),
        ("DeviceInfoMessage", 4) => Some((2, "")),
        ("DeviceInfoMessage", 5) => Some((2, "")),
        ("DeviceInfoMessage", 6) => Some((2, "")),
        ("DeviceInfoMessage", 7) => Some((0, "")),
        ("DeviceInfoMessage", 8) => Some((0, "")),
        ("DeviceInfoMessage", 9) => Some((0, "")),
        ("DeviceInfoMessage", 10) => Some((0, "")),
        ("DeviceInfoMessage", 11) => Some((0, "")),
        ("DeviceInfoMessage", 12) => Some((2, "")),
        ("DeviceInfoMessage", 13) => Some((0, "")),
        ("DeviceInfoMessage", 14) => Some((0, "")),
        ("DeviceInfoMessage", 15) => Some((0, "")),
        ("DeviceInfoMessage", 16) => Some((2, "")),
        ("DeviceInfoMessage", 17) => Some((0, "")),
        ("DeviceInfoMessage", 19) => Some((2, "")),
        ("DeviceInfoMessage", 20) => Some((2, "")),
        ("DeviceInfoMessage", 21) => Some((0, "")),
        ("DeviceInfoMessage", 22) => Some((0, "")),
        ("DeviceInfoMessage", 23) => Some((0, "")),
        ("DeviceInfoMessage", 24) => Some((0, "")),
        ("DeviceInfoMessage", 25) => Some((2, "")),
        ("DeviceInfoMessage", 26) => Some((2, "")),
        ("DeviceInfoMessage", 27) => Some((2, "")),
        ("DeviceInfoMessage", 28) => Some((2, "DeviceInfoMessage")),
        ("DeviceInfoMessage", 29) => Some((0, "")),
        ("DeviceInfoMessage", 30) => Some((0, "")),
        ("DeviceInfoMessage", 31) => Some((2, "")),
        ("DeviceInfoMessage", 32) => Some((2, "")),
        ("DeviceInfoMessage", 33) => Some((2, "")),
        ("DeviceInfoMessage", 34) => Some((2, "")),
        ("DeviceInfoMessage", 35) => Some((2, "")),
        ("DeviceInfoMessage", 36) => Some((2, "")),
        ("DeviceInfoMessage", 37) => Some((0, "")),
        ("DeviceInfoMessage", 38) => Some((0, "")),
        ("DeviceInfoMessage", 39) => Some((2, "")),
        ("DeviceInfoMessage", 40) => Some((0, "")),
        ("DeviceInfoMessage", 41) => Some((2, "")),
        ("DeviceInfoMessage", 42) => Some((2, "")),
        ("DeviceInfoMessage", 43) => Some((2, "")),
        ("DeviceInfoMessage", 44) => Some((2, "DeviceInfoMessage")),
        ("DeviceInfoMessage", 45) => Some((0, "")),
        ("DeviceInfoMessage", 46) => Some((0, "")),
        ("DeviceInfoMessage", 47) => Some((0, "")),
        ("DeviceInfoMessage", 49) => Some((0, "")),
        ("DeviceInfoMessage", 50) => Some((2, "")),
        ("DeviceInfoMessage", 51) => Some((0, "")),
        ("DeviceInfoMessage", 52) => Some((0, "")),
        ("GenericMessage", 1) => Some((2, "")),
        ("GenericMessage", 2) => Some((2, "")),
        ("GetVolumeMessage", 1) => Some((2, "")),
        ("GetVolumeResultMessage", 1) => Some((5, "")),
        ("KeyboardMessage", 1) => Some((0, "")),
        ("KeyboardMessage", 3) => Some((2, "TextEditingAttributes")),
        ("KeyboardMessage", 4) => Some((2, "")),
        ("LanguageOption", 1) => Some((0, "")),
        ("LanguageOption", 2) => Some((2, "")),
        ("LanguageOption", 3) => Some((2, "")),
        ("LanguageOption", 4) => Some((2, "")),
        ("LanguageOption", 5) => Some((2, "")),
        ("LanguageOptionGroup", 1) => Some((0, "")),
        ("LanguageOptionGroup", 2) => Some((2, "LanguageOption")),
        ("LanguageOptionGroup", 3) => Some((2, "LanguageOption")),
        ("ModifyOutputContextRequestMessage", 1) => Some((0, "")),
        ("ModifyOutputContextRequestMessage", 2) => Some((2, "")),
        ("ModifyOutputContextRequestMessage", 3) => Some((2, "")),
        ("ModifyOutputContextRequestMessage", 4) => Some((2, "")),
        ("ModifyOutputContextRequestMessage", 5) => Some((2, "")),
        ("ModifyOutputContextRequestMessage", 6) => Some((2, "")),
        ("ModifyOutputContextRequestMessage", 7) => Some((2, "")),
        ("NotificationMessage", 1) => Some((2, "")),
        ("NotificationMessage", 2) => Some((2, "")),
        ("NowPlayingClient", 1) => Some((0, "")),
        ("NowPlayingClient", 2) => Some((2, "")),
        ("NowPlayingClient", 3) => Some((2, "")),
        ("NowPlayingClient", 4) => Some((0, "")),
        ("NowPlayingClient", 5) => Some((0, "")),
        ("NowPlayingClient", 7) => Some((2, "")),
        ("NowPlayingClient", 8) => Some((2, "")),
        ("NowPlayingInfo", 1) => Some((2, "")),
        ("NowPlayingInfo", 2) => Some((2, "")),
        ("NowPlayingInfo", 3) => Some((1, "")),
        ("NowPlayingInfo", 4) => Some((1, "")),
        ("NowPlayingInfo", 5) => Some((5, "")),
        ("NowPlayingInfo", 6) => Some((0, "")),
        ("NowPlayingInfo", 7) => Some((0, "")),
        ("NowPlayingInfo", 8) => Some((1, "")),
        ("NowPlayingInfo", 9) => Some((2, "")),
        ("NowPlayingInfo", 10) => Some((0, "")),
        ("NowPlayingInfo", 11) => Some((0, "")),
        ("NowPlayingInfo", 12) => Some((0, "")),
        ("NowPlayingInfo", 13) => Some((0, "")),
        ("NowPlayingInfo", 14) => Some((2, "")),
        ("NowPlayingInfo", 15) => Some((2, "")),
        ("NowPlayingInfo", 16) => Some((2, "")),
        ("NowPlayingInfo", 17) => Some((0, "")),
        ("NowPlayingInfo", 18) => Some((0, "")),
        ("NowPlayingPlayer", 1) => Some((2, "")),
        ("NowPlayingPlayer", 2) => Some((2, "")),
        ("NowPlayingPlayer", 3) => Some((0, "")),
        ("NowPlayingPlayer", 4) => Some((0, "")),
        ("NowPlayingPlayer", 5) => Some((0, "")),
        ("NowPlayingPlayer", 6) => Some((0, "")),
        ("NowPlayingPlayer", 7) => Some((2, "")),
        ("Origin", 1) => Some((0, "")),
        ("Origin", 2) => Some((2, "")),
        ("Origin", 3) => Some((0, "")),
        ("Origin", 4) => Some((2, "DeviceInfoMessage")),
        ("Origin", 5) => Some((0, "")),
        ("OriginClientPropertiesMessage", 1) => Some((1, "")),
        ("PlaybackQueue", 1) => Some((0, "")),
        ("PlaybackQueue", 2) => Some((2, "ContentItem")),
        ("PlaybackQueue", 3) => Some((2, "PlaybackQueueContext")),
        ("PlaybackQueue", 4) => Some((2, "")),
        ("PlaybackQueue", 5) => Some((2, "PlayerPath")),
        ("PlaybackQueue", 6) => Some((0, "")),
        ("PlaybackQueue", 7) => Some((2, "")),
        ("PlaybackQueueCapabilities", 1) => Some((0, "")),
        ("PlaybackQueueCapabilities", 2) => Some((0, "")),
        ("PlaybackQueueCapabilities", 3) => Some((0, "")),
        ("PlaybackQueueContext", 1) => Some((2, "")),
        ("PlaybackQueueRequestMessage", 1) => Some((0, "")),
        ("PlaybackQueueRequestMessage", 2) => Some((0, "")),
        ("PlaybackQueueRequestMessage", 3) => Some((0, "")),
        ("PlaybackQueueRequestMessage", 4) => Some((1, "")),
        ("PlaybackQueueRequestMessage", 5) => Some((1, "")),
        ("PlaybackQueueRequestMessage", 6) => Some((0, "")),
        ("PlaybackQueueRequestMessage", 7) => Some((0, "")),
        ("PlaybackQueueRequestMessage", 8) => Some((0, "")),
        ("PlaybackQueueRequestMessage", 9) => Some((0, "")),
        ("PlaybackQueueRequestMessage", 10) => Some((2, "PlaybackQueueContext")),
        ("PlaybackQueueRequestMessage", 11) => Some((2, "")),
        ("PlaybackQueueRequestMessage", 12) => Some((2, "")),
        ("PlaybackQueueRequestMessage", 13) => Some((0, "")),
        ("PlaybackQueueRequestMessage", 14) => Some((2, "PlayerPath")),
        ("PlaybackQueueRequestMessage", 15) => Some((0, "")),
        ("PlaybackQueueRequestMessage", 16) => Some((2, "")),
        ("PlaybackQueueRequestMessage", 17) => Some((0, "")),
        ("PlayerClientPropertiesMessage", 1) => Some((2, "PlayerPath")),
        ("PlayerClientPropertiesMessage", 2) => Some((1, "")),
        ("PlayerPath", 1) => Some((2, "Origin")),
        ("PlayerPath", 2) => Some((2, "NowPlayingClient")),
        ("PlayerPath", 3) => Some((2, "NowPlayingPlayer")),
        ("PreloadedPlaybackSessionInfo", 1) => Some((2, "")),
        ("PreloadedPlaybackSessionInfo", 2) => Some((2, "")),
        ("PreloadedPlaybackSessionInfo", 3) => Some((0, "")),
        ("ProtocolMessage", 88) => Some((2, "AudioFadeMessage")),
        ("ProtocolMessage", 89) => Some((2, "AudioFadeResponseMessage")),
        ("ProtocolMessage", 21) => Some((2, "ClientUpdatesConfigMessage")),
        ("ProtocolMessage", 94) => Some((2, "ConfigureConnectionMessage")),
        ("ProtocolMessage", 39) => Some((2, "CryptoPairingMessage")),
        ("ProtocolMessage", 20) => Some((2, "DeviceInfoMessage")),
        ("ProtocolMessage", 46) => Some((2, "GenericMessage")),
        ("ProtocolMessage", 29) => Some((2, "")),
        ("ProtocolMessage", 72) => Some((2, "GetRemoteTextInputSessionMessage")),
        ("ProtocolMessage", 53) => Some((2, "GetVolumeMessage")),
        ("ProtocolMessage", 54) => Some((2, "GetVolumeResultMessage")),
        ("ProtocolMessage", 28) => Some((2, "KeyboardMessage")),
        ("ProtocolMessage", 52) => Some((2, "ModifyOutputContextRequestMessage")),
        ("ProtocolMessage", 16) => Some((2, "NotificationMessage")),
        ("ProtocolMessage", 87) => Some((2, "OriginClientPropertiesMessage")),
        ("ProtocolMessage", 37) => Some((2, "PlaybackQueueRequestMessage")),
        ("ProtocolMessage", 86) => Some((2, "PlayerClientPropertiesMessage")),
        ("ProtocolMessage", 1) => Some((0, "")),
        ("ProtocolMessage", 2) => Some((2, "")),
        ("ProtocolMessage", 3) => Some((2, "")),
        ("ProtocolMessage", 4) => Some((0, "")),
        ("ProtocolMessage", 5) => Some((0, "")),
        ("ProtocolMessage", 78) => Some((2, "")),
        ("ProtocolMessage", 85) => Some((2, "")),
        ("ProtocolMessage", 27) => Some((2, "RegisterForGameControllerEventsMessage")),
        ("ProtocolMessage", 11) => Some((2, "RegisterHIDDeviceMessage")),
        ("ProtocolMessage", 12) => Some((2, "RegisterHIDDeviceResultMessage")),
        ("ProtocolMessage", 33) => Some((2, "RegisterVoiceInputDeviceMessage")),
        ("ProtocolMessage", 34) => Some((2, "RegisterVoiceInputDeviceResponseMessage")),
        ("ProtocolMessage", 71) => Some((2, "RemoteTextInputMessage")),
        ("ProtocolMessage", 57) => Some((2, "RemoveClientMessage")),
        ("ProtocolMessage", 84) => Some((2, "RemoveEndpointsMessage")),
        ("ProtocolMessage", 70) => Some((2, "RemoveOutputDevicesMessage")),
        ("ProtocolMessage", 58) => Some((2, "RemovePlayerMessage")),
        ("ProtocolMessage", 43) => Some((2, "SendButtonEventMessage")),
        ("ProtocolMessage", 6) => Some((2, "SendCommandMessage")),
        ("ProtocolMessage", 7) => Some((2, "SendCommandResultMessage")),
        ("ProtocolMessage", 13) => Some((2, "SendHIDEventMessage")),
        ("ProtocolMessage", 47) => Some((2, "SendPackedVirtualTouchEventMessage")),
        ("ProtocolMessage", 36) => Some((2, "SendVoiceInputMessage")),
        ("ProtocolMessage", 10) => Some((2, "SetArtworkMessage")),
        ("ProtocolMessage", 42) => Some((2, "SetConnectionStateMessage")),
        ("ProtocolMessage", 75) => Some((2, "SetDefaultSupportedCommandsMessage")),
        ("ProtocolMessage", 82) => Some((2, "SetDiscoveryModeMessage")),
        ("ProtocolMessage", 44) => Some((2, "SetHiliteModeMessage")),
        ("ProtocolMessage", 50) => Some((2, "SetNowPlayingClientMessage")),
        ("ProtocolMessage", 51) => Some((2, "SetNowPlayingPlayerMessage")),
        ("ProtocolMessage", 35) => Some((2, "SetRecordingStateMessage")),
        ("ProtocolMessage", 9) => Some((2, "SetStateMessage")),
        ("ProtocolMessage", 55) => Some((2, "SetVolumeMessage")),
        ("ProtocolMessage", 30) => Some((2, "TextInputMessage")),
        ("ProtocolMessage", 38) => Some((2, "TransactionMessage")),
        ("ProtocolMessage", 59) => Some((2, "UpdateClientMessage")),
        ("ProtocolMessage", 61) => Some((2, "UpdateContentItemArtworkMessage")),
        ("ProtocolMessage", 60) => Some((2, "UpdateContentItemMessage")),
        ("ProtocolMessage", 83) => Some((2, "UpdateEndPointsMessage")),
        ("ProtocolMessage", 69) => Some((2, "UpdateOutputDeviceMessage")),
        ("ProtocolMessage", 62) => Some((2, "UpdatePlayerMessage")),
        ("ProtocolMessage", 22) => Some((2, "VolumeControlAvailabilityMessage")),
        ("ProtocolMessage", 68) => Some((2, "VolumeControlCapabilitiesDidChangeMessage")),
        ("ProtocolMessage", 56) => Some((2, "VolumeDidChangeMessage")),
        ("ProtocolMessage", 45) => Some((2, "WakeDeviceMessage")),
        ("RegisterForGameControllerEventsMessage", 1) => Some((0, "")),
        ("RegisterHIDDeviceMessage", 1) => Some((2, "VirtualTouchDeviceDescriptor")),
        ("RegisterHIDDeviceResultMessage", 1) => Some((0, "")),
        ("RegisterHIDDeviceResultMessage", 2) => Some((0, "")),
        ("RegisterVoiceInputDeviceMessage", 1) => Some((2, "VoiceInputDeviceDescriptor")),
        ("RegisterVoiceInputDeviceResponseMessage", 1) => Some((0, "")),
        ("RegisterVoiceInputDeviceResponseMessage", 2) => Some((0, "")),
        ("RemoteTextInputMessage", 1) => Some((1, "")),
        ("RemoteTextInputMessage", 2) => Some((0, "")),
        ("RemoteTextInputMessage", 3) => Some((2, "")),
        ("RemoveClientMessage", 1) => Some((2, "NowPlayingClient")),
        ("RemoveEndpointsMessage", 1) => Some((2, "")),
        ("RemoveOutputDevicesMessage", 1) => Some((2, "")),
        ("RemoveOutputDevicesMessage", 2) => Some((2, "")),
        ("RemovePlayerMessage", 1) => Some((2, "PlayerPath")),
        ("SendButtonEventMessage", 1) => Some((0, "")),
        ("SendButtonEventMessage", 2) => Some((0, "")),
        ("SendButtonEventMessage", 3) => Some((0, "")),
        ("SendCommandMessage", 1) => Some((0, "")),
        ("SendCommandMessage", 2) => Some((2, "CommandOptions")),
        ("SendCommandMessage", 3) => Some((2, "PlayerPath")),
        ("SendCommandResult", 1) => Some((2, "PlayerPath")),
        ("SendCommandResult", 2) => Some((0, "")),
        ("SendCommandResult", 3) => Some((2, "SendCommandResultStatus")),
        ("SendCommandResultMessage", 1) => Some((0, "")),
        ("SendCommandResultMessage", 2) => Some((0, "")),
        ("SendCommandResultMessage", 3) => Some((2, "")),
        ("SendCommandResultMessage", 4) => Some((2, "")),
        ("SendCommandResultMessage", 5) => Some((2, "PlayerPath")),
        ("SendCommandResultMessage", 6) => Some((2, "SendCommandResult")),
        ("SendCommandResultStatus", 1) => Some((0, "")),
        ("SendCommandResultStatus", 2) => Some((0, "")),
        ("SendCommandResultStatus", 5) => Some((2, "")),
        ("SendCommandResultStatus", 6) => Some((2, "")),
        ("SendHIDEventMessage", 1) => Some((2, "")),
        ("SendPackedVirtualTouchEventMessage", 1) => Some((2, "")),
        ("SendVoiceInputMessage", 1) => Some((2, "AudioDataBlock")),
        ("SetArtworkMessage", 1) => Some((2, "")),
        ("SetConnectionStateMessage", 1) => Some((0, "")),
        ("SetDefaultSupportedCommandsMessage", 1) => Some((2, "NowPlayingInfo")),
        ("SetDefaultSupportedCommandsMessage", 2) => Some((2, "SupportedCommands")),
        ("SetDefaultSupportedCommandsMessage", 3) => Some((2, "PlaybackQueue")),
        ("SetDefaultSupportedCommandsMessage", 4) => Some((2, "")),
        ("SetDefaultSupportedCommandsMessage", 5) => Some((2, "")),
        ("SetDefaultSupportedCommandsMessage", 6) => Some((0, "")),
        ("SetDefaultSupportedCommandsMessage", 8) => Some((2, "PlaybackQueueCapabilities")),
        ("SetDefaultSupportedCommandsMessage", 9) => Some((2, "PlayerPath")),
        ("SetDefaultSupportedCommandsMessage", 10) => Some((2, "PlaybackQueueRequestMessage")),
        ("SetDefaultSupportedCommandsMessage", 11) => Some((1, "")),
        ("SetDiscoveryModeMessage", 1) => Some((0, "")),
        ("SetDiscoveryModeMessage", 2) => Some((0, "")),
        ("SetHiliteModeMessage", 1) => Some((0, "")),
        ("SetNowPlayingClientMessage", 1) => Some((2, "NowPlayingClient")),
        ("SetNowPlayingPlayerMessage", 1) => Some((2, "PlayerPath")),
        ("SetRecordingStateMessage", 1) => Some((0, "")),
        ("SetStateMessage", 1) => Some((2, "NowPlayingInfo")),
        ("SetStateMessage", 2) => Some((2, "SupportedCommands")),
        ("SetStateMessage", 3) => Some((2, "PlaybackQueue")),
        ("SetStateMessage", 4) => Some((2, "")),
        ("SetStateMessage", 5) => Some((2, "")),
        ("SetStateMessage", 6) => Some((0, "")),
        ("SetStateMessage", 8) => Some((2, "PlaybackQueueCapabilities")),
        ("SetStateMessage", 9) => Some((2, "PlayerPath")),
        ("SetStateMessage", 10) => Some((2, "PlaybackQueueRequestMessage")),
        ("SetStateMessage", 11) => Some((1, "")),
        ("SetVolumeMessage", 1) => Some((5, "")),
        ("SetVolumeMessage", 2) => Some((2, "")),
        ("SupportedCommands", 1) => Some((2, "CommandInfo")),
        ("TextEditingAttributes", 1) => Some((2, "")),
        ("TextEditingAttributes", 2) => Some((2, "")),
        ("TextEditingAttributes", 3) => Some((2, "TextInputTraits")),
        ("TextInputMessage", 1) => Some((1, "")),
        ("TextInputMessage", 2) => Some((2, "")),
        ("TextInputMessage", 3) => Some((0, "")),
        ("TextInputTraits", 1) => Some((0, "")),
        ("TextInputTraits", 2) => Some((0, "")),
        ("TextInputTraits", 3) => Some((0, "")),
        ("TextInputTraits", 4) => Some((0, "")),
        ("TextInputTraits", 5) => Some((0, "")),
        ("TextInputTraits", 6) => Some((0, "")),
        ("TextInputTraits", 7) => Some((0, "")),
        ("TextInputTraits", 8) => Some((0, "")),
        ("TextInputTraits", 9) => Some((0, "")),
        ("TextInputTraits", 10) => Some((0, "")),
        ("TransactionKey", 1) => Some((2, "")),
        ("TransactionKey", 2) => Some((2, "")),
        ("TransactionMessage", 1) => Some((0, "")),
        ("TransactionMessage", 2) => Some((2, "TransactionPackets")),
        ("TransactionMessage", 3) => Some((2, "PlayerPath")),
        ("TransactionPacket", 1) => Some((2, "TransactionKey")),
        ("TransactionPacket", 2) => Some((2, "")),
        ("TransactionPacket", 3) => Some((2, "")),
        ("TransactionPacket", 4) => Some((0, "")),
        ("TransactionPacket", 5) => Some((0, "")),
        ("TransactionPackets", 1) => Some((2, "TransactionPacket")),
        ("UpdateClientMessage", 1) => Some((2, "NowPlayingClient")),
        ("UpdateContentItemArtworkMessage", 1) => Some((2, "ContentItem")),
        ("UpdateContentItemArtworkMessage", 2) => Some((2, "PlayerPath")),
        ("UpdateContentItemMessage", 1) => Some((2, "ContentItem")),
        ("UpdateContentItemMessage", 2) => Some((2, "PlayerPath")),
        ("UpdateEndPointsMessage", 1) => Some((2, "AVEndpointDescriptor")),
        ("UpdateEndPointsMessage", 2) => Some((0, "")),
        ("UpdateOutputDeviceMessage", 1) => Some((2, "AVOutputDeviceDescriptor")),
        ("UpdateOutputDeviceMessage", 2) => Some((2, "")),
        ("UpdateOutputDeviceMessage", 3) => Some((2, "AVOutputDeviceDescriptor")),
        ("UpdatePlayerMessage", 1) => Some((2, "PlayerPath")),
        ("VirtualTouchDeviceDescriptor", 1) => Some((0, "")),
        ("VirtualTouchDeviceDescriptor", 2) => Some((0, "")),
        ("VirtualTouchDeviceDescriptor", 3) => Some((5, "")),
        ("VirtualTouchDeviceDescriptor", 4) => Some((5, "")),
        ("VoiceInputDeviceDescriptor", 1) => Some((2, "AudioFormatSettings")),
        ("VoiceInputDeviceDescriptor", 2) => Some((2, "AudioFormatSettings")),
        ("VolumeControlAvailabilityMessage", 1) => Some((0, "")),
        ("VolumeControlAvailabilityMessage", 2) => Some((0, "")),
        ("VolumeControlCapabilitiesDidChangeMessage", 1) => {
            Some((2, "VolumeControlAvailabilityMessage"))
        }
        ("VolumeControlCapabilitiesDidChangeMessage", 3) => Some((2, "")),
        ("VolumeControlCapabilitiesDidChangeMessage", 4) => Some((2, "")),
        ("VolumeDidChangeMessage", 1) => Some((5, "")),
        ("VolumeDidChangeMessage", 2) => Some((2, "")),
        ("VolumeDidChangeMessage", 3) => Some((2, "")),
        _ => None,
    }
}
