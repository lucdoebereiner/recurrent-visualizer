Server.default.options.blockSize = 1
Server.default.options.numOutputBusChannels = 6
Server.default.reboot

s.meter


{ [SinOsc.ar(1321), SinOsc.ar(1321), SinOsc.ar(1321)] }.play

{ [SinOsc.ar(192), SinOsc.ar(1333), SinOsc.ar(800)] }.play



{ [SinOsc.ar(107), SinOsc.ar(237), SinOsc.ar(17)] }.play


{ SinOsc.ar(LFNoise1.ar(100!3).exprange(10,1000)) }.play


{ [SinOsc.ar(LFNoise1.ar(100).exprange(10,1000)), LFNoise0.ar(132), LFNoise0.ar(333)] }.play


{ [LFNoise0.ar(300), LFNoise0.ar(132), LFNoise0.ar(333)] }.play

{ [LFPulse.ar(LFNoise1.kr(2).exprange(200,500)), LFPulse.ar(132), LFPulse.ar(333)] }.play

(
{ var snd = LFPulse.ar(LFNoise1.kr(2!3).exprange(12,500));
	HPF.ar([snd[1], (snd[2] * 0.8) + (snd[2] * 0.8)],1080)
}.play
)

(
{
	var n = LFNoise1.ar(1300);
	var dev = LFNoise1.ar(1300!2) * MouseX.kr(0,10);
	[n, n + dev[0], n + dev[0]]

}.scope
)

{ [LFNoise1.ar(3), LFNoise1.ar(2), LFNoise1.ar(3)] }.play


{ [LFNoise0.ar(1300), LFNoise0.ar(1200), LFNoise0.ar(1300)] }.play


{ LPF.ar([LFNoise0.ar(131), LFNoise0.ar(321), LFNoise0.ar(193)].sign, 5) }.play


{ LPF.ar([LFNoise1.ar(131), LFNoise1.ar(321), LFNoise1.ar(193)], 1) }.play



(
~source1 = Buffer.readChannel(s,"/home/luc/Dropbox/music/trio23/02-david-230406_180449.wav", startFrame: (48000*60*5), numFrames: (48000*60*5), channels:[0]);

~source2 = Buffer.readChannel(s,"/home/luc/Dropbox/music/trio23/04-ji-230406_180449.wav", startFrame: (48000*60*5), numFrames: (48000*60*5), channels:[0]);

~source3 = Buffer.readChannel(s,"/home/luc/Dropbox/music/trio23/03-leonie-230406_180449.wav", startFrame: (48000*60*5), numFrames: (48000*60*5), channels:[0]);
)

{ [PlayBuf.ar(1, ~source1.bufnum, loop: 1), PlayBuf.ar(1, ~source2.bufnum, loop: 1), PlayBuf.ar(1, ~source3.bufnum, loop: 1)] }.play



{ LPF.ar([PlayBuf.ar(1, ~source1.bufnum, loop: 1), PlayBuf.ar(1, ~source2.bufnum, loop: 1)*4, PlayBuf.ar(1, ~source3.bufnum, loop: 1)], 100).sign }.play


b = NetAddr.new("127.0.0.1", 8000);    // create the NetAddr
b.sendMsg("/facdown", 0.95)
b.sendMsg("/facup", 0.95)
b.sendMsg("/factor", 2)
b.sendMsg("/exponent", 1.25)
b.sendMsg("/bwmode", 0)
b.sendMsg("/zoom", 0.0)
b.sendMsg("/offsety", 3)



